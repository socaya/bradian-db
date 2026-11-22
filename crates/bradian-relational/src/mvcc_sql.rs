//! G21: opt-in typed multi-table SQL sessions using G20 pinned MVCC versions.
//!
//! This frontend reuses G16's SQL parser and physical G15 row encoding but
//! exclusively owns the WAL through the G20 MvccStore. It requires existing
//! G15/G18 table pins and does not mutate durable catalog metadata.
//! It allows independent concurrent scoped sessions through one engine,
//! unlike G16's serialized global session mutex. Historical snapshots
//! are in-process only, not recoverable after restart.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::Table;
use crate::persistent_catalog::PersistentCatalog;
use crate::row_store::BaceWiring;
use crate::sql::{
    coerce, locate, qualified, Parser, SqlError, SqlResult, SqlResultSet,
};
use crate::typed_mvcc::{TypedMvccStore, TypedMvccTransaction};

/// SQL-enabled shared transaction domain. The durable catalog and the domain
/// own separate locks and WALs: existing metadata is resolved but not changed
/// by a domain transaction. DDL is intentionally unavailable in this API.
pub struct MvccSqlEngine {
    catalog: PersistentCatalog,
    domain: TypedMvccStore,
    _root: PathBuf,
    /// ADR 0170 — workspace-wide admission cap.
    global_budget: crate::workspace_usage::GlobalBudget,
}

impl MvccSqlEngine {
    pub fn open(root: impl AsRef<Path>) -> SqlResult<Self> {
        Self::open_with_limits(
            root,
            crate::runtime_limits::MvccStoreLimits::default(),
            crate::runtime_limits::CatalogLimits::default(),
        )
    }

    /// Open the opt-in G21 MVCC SQL engine with explicit admission for
    /// the underlying `MvccStore` (ADR 0058) and `PersistentCatalog`
    /// (ADR 0054). Policies are not persisted.
    pub fn open_with_limits(
        root: impl AsRef<Path>,
        mvcc_limits: crate::runtime_limits::MvccStoreLimits,
        catalog_limits: crate::runtime_limits::CatalogLimits,
    ) -> SqlResult<Self> {
        Self::open_inner(root.as_ref(), mvcc_limits, catalog_limits, None)
    }

    /// ADR 0072 — open the opt-in G21 MVCC SQL engine under BACE
    /// wiring. The wiring is cloned into both component stores at
    /// open time; the G18 lifecycle pre-flight (when present) also
    /// uses `CatalogLifecycle::open_with_bace` so the temporary
    /// coordinator reconcile matches the subsequent eager opens.
    pub fn open_with_bace(root: impl AsRef<Path>, wiring: BaceWiring) -> SqlResult<Self> {
        Self::open_with_bace_and_limits(
            root, wiring,
            crate::runtime_limits::MvccStoreLimits::default(),
            crate::runtime_limits::CatalogLimits::default(),
        )
    }

    pub fn open_with_bace_and_limits(
        root: impl AsRef<Path>, wiring: BaceWiring,
        mvcc_limits: crate::runtime_limits::MvccStoreLimits,
        catalog_limits: crate::runtime_limits::CatalogLimits,
    ) -> SqlResult<Self> {
        Self::open_inner(root.as_ref(), mvcc_limits, catalog_limits, Some(wiring))
    }

    fn open_inner(
        root: &Path,
        mvcc_limits: crate::runtime_limits::MvccStoreLimits,
        catalog_limits: crate::runtime_limits::CatalogLimits,
        wiring: Option<BaceWiring>,
    ) -> SqlResult<Self> {
        if root.join("history-domain").exists() {
            return Err(SqlError::Unsupported(
                "G23 durable history root requires HistorySqlEngine; legacy SQL ownership refused".into()
            ));
        }
        // Refuse opening a root with ANY legacy per-table data. No implicit
        // migration, dual writes, or ambiguous source of truth is allowed.
        let old_tables = root.join("tables");
        if old_tables.exists() && fs::read_dir(&old_tables)?.next().is_some() {
            return Err(SqlError::Unsupported(
                "legacy per-table storage exists; explicit verified migration is required".into(),
            ));
        }
        fs::create_dir_all(root)?;
        // G21 cannot provision missing tables. Opening a root that has
        // never been registered must fail rather than silently start a new
        // physical storage domain with zero data.
        if !root.join("catalog").is_dir() || !root.join("shared-domain").is_dir() {
            return Err(SqlError::Unsupported(
                "G21 requires an existing provisioned G15/G18 database".into(),
            ));
        }
        // A G18 lifecycle journal may contain an interrupted Pending pin.
        // Recover/reconcile it before taking ownership of the catalog and
        // domain for MVCC. The temporary coordinator releases every file
        // lock before this engine opens its own exclusive RowStore.
        if root.join("lifecycle").exists() {
            match &wiring {
                Some(w) => crate::lifecycle::CatalogLifecycle::open_with_bace(root, w.clone())
                    .map_err(|e| SqlError::Unsupported(format!(
                        "G18 lifecycle reconciliation failed: {e}"
                    )))?,
                None => crate::lifecycle::CatalogLifecycle::open(root)
                    .map_err(|e| SqlError::Unsupported(format!(
                        "G18 lifecycle reconciliation failed: {e}"
                    )))?,
            };
        }
        let (catalog, domain) = match wiring {
            Some(w) => {
                let catalog = PersistentCatalog::open_with_bace_and_limits(
                    root.join("catalog"), w.clone(), catalog_limits,
                )?;
                let definitions = catalog.snapshot().tables()
                    .map(|table| (table.id, table.clone()))
                    .collect::<BTreeMap<_,_>>();
                let domain = TypedMvccStore::open_with_bace_and_limits(
                    root.join("shared-domain"), &definitions, w, mvcc_limits,
                )?;
                (catalog, domain)
            }
            None => {
                let catalog = PersistentCatalog::open_with_limits(root.join("catalog"), catalog_limits)?;
                let definitions = catalog.snapshot().tables()
                    .map(|table| (table.id, table.clone()))
                    .collect::<BTreeMap<_,_>>();
                let domain = TypedMvccStore::open_with_limits(
                    root.join("shared-domain"), &definitions, mvcc_limits,
                )?;
                (catalog, domain)
            }
        };
        Ok(Self {
            catalog, domain, _root: root.to_owned(),
            global_budget: crate::workspace_usage::GlobalBudget::default(),
        })
    }

    fn resolve(&self, parts: &[String]) -> SqlResult<Table> {
        qualified(parts, 3)?;
        let snapshot = self.catalog.snapshot();
        let database = snapshot.databases().find(|db| db.name == parts[0])
            .ok_or_else(|| SqlError::Resolution(format!("unknown database {}", parts[0])))?;
        let schema = snapshot.schemas()
            .find(|s| s.database == database.id && s.name == parts[1])
            .ok_or_else(|| SqlError::Resolution(format!("unknown schema {}", parts[1])))?;
        let table = snapshot.tables()
            .find(|t| t.schema == schema.id && t.name == parts[2])
            .ok_or_else(|| SqlError::Resolution(format!("unknown table {}", parts[2])))?;
        if self.domain.table(table.id) != Some(table) {
            return Err(SqlError::Unsupported(format!(
                "table {} has no matching G15/G18 MVCC domain pin", table.id.0
            )));
        }
        Ok(table.clone())
    }

    /// Only schema pins already recorded by G15/G18 may be used.
    /// ADR 0170 — attach a workspace-wide admission cap.
    pub fn with_global_budget(mut self, budget: crate::workspace_usage::GlobalBudget) -> Self {
        self.global_budget = budget;
        self
    }

    /// ADR 0170 — introspect the current cap.
    pub fn global_budget(&self) -> crate::workspace_usage::GlobalBudget {
        self.global_budget
    }

    pub fn registered_table_ids(&self) -> Vec<crate::TableId> {
        self.domain.active_tables()
    }

    /// Observe the in-memory snapshot retention state.
    pub fn mvcc_stats(&self) -> crate::mvcc::MvccStats {
        self.domain.stats()
    }

    pub fn last_lsn(&self) -> u64 { self.domain.last_lsn() }
    pub fn checkpoint(&self) -> SqlResult<u64> { Ok(self.domain.checkpoint()?) }

    /// Every callback owns a separate pinned MVCC snapshot. BEGIN
    /// opens a domain-wide row snapshot; COMMIT publishes one WAL frame.
    /// Leaving with a pending transaction discards all staged modifications.
    pub fn with_session<R>(
        &self,
        callback: impl FnOnce(&mut MvccSqlSession<'_>) -> SqlResult<R>,
    ) -> SqlResult<R> {
        let mut session = MvccSqlSession {
            engine: self,
            active: None,
            last_commit_lsn: None,
            budget: None,
            policy: None,
            authz: None,
        };
        callback(&mut session)
    }

    /// ADR 0146 — session with a caller-supplied per-query
    /// budget (ADR 0141 `BudgetTracker`). The SELECT path
    /// charges scanned rows + checks the wall-clock
    /// deadline; breach returns `SqlError::Budget`. Other
    /// paths are unchanged.
    pub fn with_session_budget<R>(
        &self, budget: crate::query_budget::QueryBudget,
        callback: impl FnOnce(&mut MvccSqlSession<'_>) -> SqlResult<R>,
    ) -> SqlResult<R> {
        let mut session = MvccSqlSession {
            engine: self,
            active: None,
            last_commit_lsn: None,
            budget: Some(crate::query_budget::BudgetTracker::new(budget)),
            policy: None,
            authz: None,
        };
        callback(&mut session)
    }

    /// ADR 0152 — session with a budget + cooperative
    /// cancellation handle.
    pub fn with_session_budget_and_cancellation<R>(
        &self,
        budget: crate::query_budget::QueryBudget,
        cancellation: crate::query_budget::CancellationHandle,
        callback: impl FnOnce(&mut MvccSqlSession<'_>) -> SqlResult<R>,
    ) -> SqlResult<R> {
        let tracker = crate::query_budget::BudgetTracker::new(budget)
            .with_cancellation(cancellation);
        let mut session = MvccSqlSession {
            engine: self,
            active: None,
            last_commit_lsn: None,
            budget: Some(tracker),
            policy: None,
            authz: None,
        };
        callback(&mut session)
    }

    /// ADR 0156 — session with budget + policy together.
    pub fn with_session_budget_and_policy<R>(
        &self,
        budget: crate::query_budget::QueryBudget,
        policy: crate::access_policy::TableAccessPolicy,
        authz: bradian_core::AuthorizationContext,
        callback: impl FnOnce(&mut MvccSqlSession<'_>) -> SqlResult<R>,
    ) -> SqlResult<R> {
        let mut session = MvccSqlSession {
            engine: self,
            active: None,
            last_commit_lsn: None,
            budget: Some(crate::query_budget::BudgetTracker::new(budget)),
            policy: Some(policy),
            authz: Some(authz),
        };
        callback(&mut session)
    }

    /// ADR 0154 — session with an ADR 0123 row/column/
    /// tenant policy. SELECT output filtered by
    /// `TableAccessPolicy::allows_row`.
    pub fn with_session_policy<R>(
        &self,
        policy: crate::access_policy::TableAccessPolicy,
        authz: bradian_core::AuthorizationContext,
        callback: impl FnOnce(&mut MvccSqlSession<'_>) -> SqlResult<R>,
    ) -> SqlResult<R> {
        let mut session = MvccSqlSession {
            engine: self,
            active: None,
            last_commit_lsn: None,
            budget: None,
            policy: Some(policy),
            authz: Some(authz),
        };
        callback(&mut session)
    }
}

/// A scoped SQL session that may access several registered catalog tables.
pub struct MvccSqlSession<'a> {
    engine: &'a MvccSqlEngine,
    active: Option<TypedMvccTransaction<'a>>,
    last_commit_lsn: Option<u64>,
    /// ADR 0146 — optional per-query budget tracker.
    budget: Option<crate::query_budget::BudgetTracker>,
    /// ADR 0154 — optional row/column/tenant policy.
    policy: Option<crate::access_policy::TableAccessPolicy>,
    /// ADR 0154 — authorization context paired with policy.
    authz: Option<bradian_core::AuthorizationContext>,
}

impl<'a> MvccSqlSession<'a> {
    pub fn in_transaction(&self) -> bool { self.active.is_some() }
    pub fn last_commit_lsn(&self) -> Option<u64> { self.last_commit_lsn }
    pub fn snapshot_lsn(&self) -> Option<u64> {
        self.active.as_ref().map(TypedMvccTransaction::start_lsn)
    }

    /// A failed statement aborts the current transaction; subsequent COMMIT
    /// cannot accidentally publish earlier staged writes.
    pub fn execute(&mut self, sql: &str) -> SqlResult<SqlResultSet> {
        let result = self.execute_inner(sql);
        if result.is_err() { self.active = None; }
        result
    }

    fn execute_inner(&mut self, sql: &str) -> SqlResult<SqlResultSet> {
        // ADR 0170 — workspace-wide admission cap. Populate
        // the MVCC fields directly from TypedMvccStore::stats()
        // since the underlying MvccStore is private.
        let mut usage = crate::workspace_usage::WorkspaceUsage::for_catalog(&self.engine.catalog);
        let stats = self.engine.domain.stats();
        usage.mvcc_distinct_keys = Some(stats.keys_retained);
        usage.mvcc_total_versions = Some(stats.versions_retained);
        self.engine.global_budget.check(&usage)?;
        let mut p = Parser::new(sql)?;
        let command = p.word()?;
        // ADR 0150 — one iteration = one executed statement.
        if let Some(b) = self.budget.as_mut() { b.charge_iteration()?; }
        match command.as_str() {
            "begin" => {
                p.consume_word("transaction");
                p.end()?;
                if self.active.is_some() {
                    return Err(SqlError::Unsupported("nested BEGIN is unsupported".into()));
                }
                let tx: TypedMvccTransaction<'a> = self.engine.domain.begin();
                self.active = Some(tx);
                Ok(SqlResultSet::Command { affected: 0 })
            }
            "commit" => {
                p.end()?;
                let tx = self.active.take().ok_or_else(||
                    SqlError::Unsupported("COMMIT without BEGIN".into()))?;
                let lsn = tx.commit()?;
                self.last_commit_lsn = Some(lsn);
                Ok(SqlResultSet::Command { affected: 0 })
            }
            "rollback" => {
                p.end()?;
                let tx = self.active.take().ok_or_else(||
                    SqlError::Unsupported("ROLLBACK without BEGIN".into()))?;
                tx.rollback();
                Ok(SqlResultSet::Command { affected: 0 })
            }
            "insert" | "select" | "update" | "delete" => {
                // Resolve and parse before applying any change. On any error,
                // execute() drops the pending transaction entirely.
                let tx = self.active.as_mut().ok_or_else(||
                    SqlError::Unsupported("BEGIN required before transactional DML".into()))?;
                match command.as_str() {
                    "insert" => {
                        // ADR 0147 — deadline check before mutation.
                        if let Some(b) = self.budget.as_ref() { b.check_deadline()?; }
                        p.expect("into")?;
                        let table = self.engine.resolve(&p.name()?)?;
                        let names = p.list(|parser| parser.word())?;
                        p.expect("values")?;
                        let values = p.list(|parser| parser.literal())?;
                        p.end()?;
                        if names.len() != values.len() || names.len() != table.columns.len() {
                            return Err(SqlError::Unsupported("INSERT must list every table column exactly once".into()));
                        }
                        let mut fields = BTreeMap::new();
                        for (name, value) in names.into_iter().zip(values) {
                            locate(&table, &name)?;
                            if fields.insert(name.clone(), value).is_some() {
                                return Err(SqlError::Syntax(format!("duplicate column {name}")));
                            }
                        }
                        let row = table.columns.iter().map(|column| {
                            let lit = fields.remove(&column.name)
                                .ok_or_else(|| SqlError::Resolution("missing column".into()))?;
                            coerce(&lit, column)
                        }).collect::<SqlResult<Vec<_>>>()?;
                        // ADR 0182 — per-column CHECK.
                        crate::sql::check_row(&table, &row)?;
                        // ADR 0187 — FK parent-exists through
                        // the active MVCC-SQL transaction.
                        crate::sql::fk_check_child_row_tx(
                            &self.engine.catalog, &table, &row, tx)?;
                        // ADR 0179 — UNIQUE enforcement at
                        // MVCC-SQL INSERT mirrors the pattern
                        // across the other three engines.
                        if !table.unique_keys.is_empty() {
                            let existing = tx.scan(table.id)?;
                            crate::unique_check::UniqueConstraintChecker::new(&table)
                                .check(&row, existing)?;
                        }
                        tx.insert(table.id, &row)?;
                        // ADR 0147 rows + ADR 0149 bytes.
                        if let Some(b) = self.budget.as_mut() {
                            b.charge_rows(1)?;
                            b.charge_bytes(crate::query_budget::row_bytes(&row))?;
                        }
                        Ok(SqlResultSet::Command { affected: 1 })
                    }
                    "select" => {
                        // ADR 0146 — deadline check before scan.
                        if let Some(b) = self.budget.as_ref() { b.check_deadline()?; }
                        // ADR 0188 — JSON path projections.
                        let caller_wildcard = p.consume_symbol('*');
                        let requested_projections = if caller_wildcard { None } else {
                            let mut out = vec![crate::sql::parse_projection(&mut p)?];
                            while p.consume_symbol(',') {
                                out.push(crate::sql::parse_projection(&mut p)?);
                            }
                            Some(out)
                        };
                        p.expect("from")?;
                        let table = self.engine.resolve(&p.name()?)?;
                        let mut projections = requested_projections.unwrap_or_else(||
                            table.columns.iter()
                                .map(|c| crate::sql::Projection::Column(c.name.clone()))
                                .collect());
                        // ADR 0155 — column-level policy filtering.
                        if let Some(p_) = self.policy.as_ref() {
                            if p_.column_level.is_configured() {
                                if caller_wildcard {
                                    projections.retain(|pj| p_.column_level.allows_read(pj.source_column()));
                                } else {
                                    for pj in &projections {
                                        if !p_.column_level.allows_read(pj.source_column()) {
                                            return Err(SqlError::Policy(
                                                crate::access_policy::PolicyError::ColumnDenied {
                                                    column: pj.source_column().into(),
                                                }));
                                        }
                                    }
                                }
                            }
                        }
                        let positions: Vec<usize> = projections.iter()
                            .map(|pj| locate(&table, pj.source_column()))
                            .collect::<SqlResult<Vec<_>>>()?;
                        // ADR 0190 — WHERE accepts PK equality
                        // or JSON-path equality (full scan +
                        // post-filter).
                        let where_clause = if p.next_is_word("where") {
                            Some(crate::sql::parse_select_where(&mut p, &table)?)
                        } else { None };
                        let rows: Vec<Vec<_>> = match &where_clause {
                            Some(crate::sql::SelectWhere::KeyPredicate(key)) =>
                                tx.get(table.id, key)?.into_iter().collect(),
                            Some(crate::sql::SelectWhere::JsonPathPredicate { .. }) =>
                                tx.scan(table.id)?,
                            None => tx.scan(table.id)?,
                        };
                        p.end()?;
                        // ADR 0146 — charge rows after scan.
                        // ADR 0148 — charge materialized bytes.
                        if let Some(b) = self.budget.as_mut() {
                            b.charge_rows(rows.len() as u64)?;
                            b.charge_bytes(crate::query_budget::rows_bytes(&rows))?;
                            b.check_deadline()?;
                        }
                        // ADR 0154 — row/column/tenant policy filter.
                        let rows: Vec<Vec<_>> = if let (Some(p_), Some(a)) =
                            (self.policy.as_ref(), self.authz.as_ref())
                        {
                            let schema_cols: Vec<String> = table.columns.iter()
                                .map(|c| c.name.clone()).collect();
                            let mut kept = Vec::with_capacity(rows.len());
                            for row in rows {
                                if p_.allows_row(&row, &schema_cols, a)? {
                                    kept.push(row);
                                }
                            }
                            kept
                        } else { rows };
                        // ADR 0190 — post-scan JSON-path filter.
                        let rows = match &where_clause {
                            Some(crate::sql::SelectWhere::JsonPathPredicate { projection, op, literal }) =>
                                crate::sql::apply_where_filter(rows, &table, projection, *op, literal)?,
                            _ => rows,
                        };
                        let columns: Vec<String> = projections.iter().map(|pj| pj.label()).collect();
                        Ok(SqlResultSet::Rows {
                            columns,
                            rows: rows.into_iter().map(|row: Vec<_>|
                                positions.iter().zip(&projections).map(|(pos, pj)|
                                    crate::sql::apply_projection(pj, row[*pos].clone())
                                ).collect::<SqlResult<Vec<_>>>()
                            ).collect::<SqlResult<Vec<Vec<_>>>>()?,
                        })
                    }
                    "update" => {
                        // ADR 0147 — deadline check before mutation.
                        if let Some(b) = self.budget.as_ref() { b.check_deadline()?; }
                        let table = self.engine.resolve(&p.name()?)?;
                        p.expect("set")?;
                        // ADR 0192 — bare + JSON-path assignments.
                        let updates = p.path_assignments()?;
                        // ADR 0199 — WHERE accepts PK equality
                        // or JSON-path predicate.
                        let where_clause = crate::sql::parse_select_where(&mut p, &table)?;
                        p.end()?;
                        let mut replacements = BTreeMap::new();
                        let mut json_assignments: Vec<(usize, Vec<crate::sql::JsonSegment>, bool, crate::sql::Literal)> = Vec::new();
                        let mut json_removes: Vec<(usize, Vec<crate::sql::JsonSegment>)> = Vec::new();
                        for change in updates {
                            match change {
                                crate::sql::SetAssignment::Column { column, value } => {
                                    if table.primary_key.columns.contains(&column) {
                                        return Err(SqlError::Unsupported("primary key UPDATE not supported".into()));
                                    }
                                    let position = locate(&table, &column)?;
                                    if replacements.insert(position, coerce(&value, &table.columns[position])?).is_some() {
                                        return Err(SqlError::Syntax("duplicate SET column".into()));
                                    }
                                }
                                crate::sql::SetAssignment::JsonPath { column, path, as_text, value } => {
                                    if table.primary_key.columns.contains(&column) {
                                        return Err(SqlError::Unsupported("primary key UPDATE not supported".into()));
                                    }
                                    let position = locate(&table, &column)?;
                                    json_assignments.push((position, path, as_text, value));
                                }
                                crate::sql::SetAssignment::JsonRemove { column, path } => {
                                    if table.primary_key.columns.contains(&column) {
                                        return Err(SqlError::Unsupported("primary key UPDATE not supported".into()));
                                    }
                                    let position = locate(&table, &column)?;
                                    json_removes.push((position, path));
                                }
                            }
                        }
                        let pk_positions: Vec<usize> = table.primary_key.columns.iter()
                            .map(|c| locate(&table, c))
                            .collect::<SqlResult<Vec<_>>>()?;
                        let targets: Vec<(Vec<crate::typed_table::Value>, Vec<crate::typed_table::Value>)> = match &where_clause {
                            crate::sql::SelectWhere::KeyPredicate(pk) => match tx.get(table.id, pk)? {
                                Some(row) => vec![(pk.clone(), row)],
                                None => Vec::new(),
                            },
                            crate::sql::SelectWhere::JsonPathPredicate { projection, op, literal } => {
                                let filtered = crate::sql::apply_where_filter(
                                    tx.scan(table.id)?, &table, projection, *op, literal)?;
                                filtered.into_iter().map(|row| {
                                    let pk: Vec<_> = pk_positions.iter()
                                        .map(|p| row[*p].clone()).collect();
                                    (pk, row)
                                }).collect()
                            }
                        };
                        let mut written_bytes: u64 = 0;
                        let mut affected: u64 = 0;
                        for (pk, mut row) in targets {
                            for (position, value) in &replacements { row[*position] = value.clone(); }
                            for (pos, path, as_text, literal) in &json_assignments {
                                let col_name = table.columns[*pos].name.clone();
                                row[*pos] = crate::sql::apply_json_path_mutation(
                                    &col_name, &row[*pos], path, *as_text, literal)?;
                            }
                            // ADR 0200 — JSON removes.
                            for (pos, path) in &json_removes {
                                let col_name = table.columns[*pos].name.clone();
                                row[*pos] = crate::sql::apply_json_path_remove(
                                    &col_name, &row[*pos], path)?;
                            }
                            crate::sql::check_row(&table, &row)?;
                            crate::sql::fk_check_child_row_tx(
                                &self.engine.catalog, &table, &row, tx)?;
                            if !table.unique_keys.is_empty() {
                                let others = tx.scan(table.id)?.into_iter()
                                    .filter(|r| pk_positions.iter().zip(pk.iter())
                                        .any(|(pos, k)| &r[*pos] != k));
                                crate::unique_check::UniqueConstraintChecker::new(&table)
                                    .check(&row, others)?;
                            }
                            written_bytes = written_bytes.saturating_add(
                                crate::query_budget::row_bytes(&row));
                            tx.replace(table.id, &row)?;
                            affected += 1;
                        }
                        if let Some(b) = self.budget.as_mut() {
                            b.charge_rows(affected)?;
                            if affected > 0 { b.charge_bytes(written_bytes)?; }
                        }
                        Ok(SqlResultSet::Command { affected })
                    }
                    "delete" => {
                        // ADR 0147 — deadline check before mutation.
                        if let Some(b) = self.budget.as_ref() { b.check_deadline()?; }
                        p.expect("from")?;
                        let table = self.engine.resolve(&p.name()?)?;
                        // ADR 0199 — PK equality or JSON path.
                        let where_clause = crate::sql::parse_select_where(&mut p, &table)?;
                        p.end()?;
                        let pk_positions: Vec<usize> = table.primary_key.columns.iter()
                            .map(|c| locate(&table, c))
                            .collect::<SqlResult<Vec<_>>>()?;
                        let targets: Vec<(Vec<crate::typed_table::Value>, Vec<crate::typed_table::Value>)> = match &where_clause {
                            crate::sql::SelectWhere::KeyPredicate(pk) => match tx.get(table.id, pk)? {
                                Some(row) => vec![(pk.clone(), row)],
                                None => Vec::new(),
                            },
                            crate::sql::SelectWhere::JsonPathPredicate { projection, op, literal } => {
                                let filtered = crate::sql::apply_where_filter(
                                    tx.scan(table.id)?, &table, projection, *op, literal)?;
                                filtered.into_iter().map(|row| {
                                    let pk: Vec<_> = pk_positions.iter()
                                        .map(|p| row[*p].clone()).collect();
                                    (pk, row)
                                }).collect()
                            }
                        };
                        let mut deleted_bytes: u64 = 0;
                        let mut affected: u64 = 0;
                        for (pk, row) in targets {
                            crate::sql::fk_apply_parent_delete_tx(
                                &self.engine.catalog, &table, &pk, tx)?;
                            tx.delete(table.id, &pk)?;
                            deleted_bytes = deleted_bytes.saturating_add(crate::query_budget::row_bytes(&row));
                            affected += 1;
                        }
                        if let Some(b) = self.budget.as_mut() {
                            b.charge_rows(affected)?;
                            if affected > 0 { b.charge_bytes(deleted_bytes)?; }
                        }
                        Ok(SqlResultSet::Command { affected })
                    }
                    _ => unreachable!(),
                }
            }
            _ => Err(SqlError::Unsupported(format!(
                "shared-domain SQL sessions do not support {command}"
            ))),
        }
    }
}
