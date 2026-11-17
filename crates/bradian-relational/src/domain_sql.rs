//! G16 explicit SQL frontend for G15 shared-WAL multi-table transactions.
//!
//! This frontend is intentionally separate from the legacy per-table SqlEngine.
//! Existing G14 table files are NEVER silently migrated. All transactions
//! use one registered shared domain, committing row changes across tables in
//! exactly one BRWL batch. Catalog DDL remains a separate transactional scope.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;

use crate::Table;
use crate::persistent_catalog::PersistentCatalog;
use crate::row_store::BaceWiring;
use crate::sql::{
    coerce, key_predicate, locate, qualified, Parser, SqlError, SqlResult, SqlResultSet,
};
use crate::transaction_domain::{MultiTableStore, MultiTableTransaction};

/// SQL-enabled shared transaction domain. The durable catalog and the domain
/// own separate locks and WALs: existing metadata is resolved but not changed
/// by a domain transaction. DDL is intentionally unavailable in this API.
pub struct DomainSqlEngine {
    catalog: PersistentCatalog,
    domain: MultiTableStore,
    operation: Mutex<()>,
    _root: PathBuf,
}

impl DomainSqlEngine {
    pub fn open(root: impl AsRef<Path>) -> SqlResult<Self> {
        Self::open_with_limits(
            root,
            crate::runtime_limits::DomainLimits::default(),
            crate::runtime_limits::CatalogLimits::default(),
        )
    }

    /// Open the shared-domain SQL engine with explicit admission for the
    /// underlying `MultiTableStore` (ADR 0056) and `PersistentCatalog`
    /// (ADR 0054). Policies are not persisted; they bound live decoded
    /// state only. See also `MvccSqlEngine::open_with_limits`.
    pub fn open_with_limits(
        root: impl AsRef<Path>,
        domain_limits: crate::runtime_limits::DomainLimits,
        catalog_limits: crate::runtime_limits::CatalogLimits,
    ) -> SqlResult<Self> {
        Self::open_inner(root.as_ref(), domain_limits, catalog_limits, None)
    }

    /// ADR 0071 — open the shared-domain SQL engine under BACE wiring.
    /// The wiring is cloned into both component stores at open time;
    /// no wiring is stored on the engine itself.
    pub fn open_with_bace(root: impl AsRef<Path>, wiring: BaceWiring) -> SqlResult<Self> {
        Self::open_with_bace_and_limits(
            root, wiring,
            crate::runtime_limits::DomainLimits::default(),
            crate::runtime_limits::CatalogLimits::default(),
        )
    }

    pub fn open_with_bace_and_limits(
        root: impl AsRef<Path>, wiring: BaceWiring,
        domain_limits: crate::runtime_limits::DomainLimits,
        catalog_limits: crate::runtime_limits::CatalogLimits,
    ) -> SqlResult<Self> {
        Self::open_inner(root.as_ref(), domain_limits, catalog_limits, Some(wiring))
    }

    fn open_inner(
        root: &Path,
        domain_limits: crate::runtime_limits::DomainLimits,
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
        let (catalog, domain) = match wiring {
            Some(w) => (
                PersistentCatalog::open_with_bace_and_limits(
                    root.join("catalog"), w.clone(), catalog_limits,
                )?,
                MultiTableStore::open_with_bace_and_limits(
                    root.join("shared-domain"), w, domain_limits,
                )?,
            ),
            None => (
                PersistentCatalog::open_with_limits(root.join("catalog"), catalog_limits)?,
                MultiTableStore::open_with_limits(root.join("shared-domain"), domain_limits)?,
            ),
        };
        Ok(Self { catalog, domain, operation: Mutex::new(()), _root: root.to_owned() })
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
        Ok(table.clone())
    }

    /// Explicitly bind one catalog table to this shared WAL, including a
    /// persisted immutable schema pin. Re-running this for the same schema
    /// is idempotent; incompatible definitions are rejected.
    pub fn register_catalog_table(&self, qualified_name: &str) -> SqlResult<u64> {
        let _guard = self.operation.lock();
        let mut parser = Parser::new(qualified_name)?;
        let name = parser.name()?;
        parser.end()?;
        let table = self.resolve(&name)?;
        Ok(self.domain.register_table(table)?)
    }

    pub fn last_lsn(&self) -> u64 { self.domain.last_lsn() }
    pub fn checkpoint(&self) -> SqlResult<u64> { Ok(self.domain.checkpoint()?) }

    /// All statements in the callback share one mutable session. BEGIN
    /// opens a domain-wide row snapshot; COMMIT publishes one WAL frame.
    /// Leaving with a pending transaction discards all staged modifications.
    pub fn with_session<R>(
        &self,
        callback: impl FnOnce(&mut DomainSqlSession<'_>) -> SqlResult<R>,
    ) -> SqlResult<R> {
        let _guard = self.operation.lock();
        let mut session = DomainSqlSession {
            engine: self,
            active: None,
            last_commit_lsn: None,
            budget: None,
        };
        callback(&mut session)
    }

    /// ADR 0145 — session with a caller-supplied per-query
    /// budget (ADR 0141 `BudgetTracker`). The SELECT path
    /// charges scanned rows + checks the wall-clock
    /// deadline; breach returns `SqlError::Budget`. Other
    /// paths are unchanged.
    pub fn with_session_budget<R>(
        &self, budget: crate::query_budget::QueryBudget,
        callback: impl FnOnce(&mut DomainSqlSession<'_>) -> SqlResult<R>,
    ) -> SqlResult<R> {
        let _guard = self.operation.lock();
        let mut session = DomainSqlSession {
            engine: self,
            active: None,
            last_commit_lsn: None,
            budget: Some(crate::query_budget::BudgetTracker::new(budget)),
        };
        callback(&mut session)
    }
}

/// A scoped SQL session that may access several registered catalog tables.
pub struct DomainSqlSession<'a> {
    engine: &'a DomainSqlEngine,
    active: Option<MultiTableTransaction<'a>>,
    last_commit_lsn: Option<u64>,
    /// ADR 0145 — optional per-query budget tracker.
    budget: Option<crate::query_budget::BudgetTracker>,
}

impl<'a> DomainSqlSession<'a> {
    pub fn in_transaction(&self) -> bool { self.active.is_some() }
    pub fn last_commit_lsn(&self) -> Option<u64> { self.last_commit_lsn }

    /// A failed statement aborts the current transaction; subsequent COMMIT
    /// cannot accidentally publish earlier staged writes.
    pub fn execute(&mut self, sql: &str) -> SqlResult<SqlResultSet> {
        let result = self.execute_inner(sql);
        if result.is_err() { self.active = None; }
        result
    }

    fn execute_inner(&mut self, sql: &str) -> SqlResult<SqlResultSet> {
        let mut p = Parser::new(sql)?;
        let command = p.word()?;
        match command.as_str() {
            "begin" => {
                p.consume_word("transaction");
                p.end()?;
                if self.active.is_some() {
                    return Err(SqlError::Unsupported("nested BEGIN is unsupported".into()));
                }
                let tx: MultiTableTransaction<'a> = self.engine.domain.begin_transaction()?;
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
                        tx.insert(table.id, &row)?;
                        if let Some(b) = self.budget.as_mut() { b.charge_rows(1)?; }
                        Ok(SqlResultSet::Command { affected: 1 })
                    }
                    "select" => {
                        // ADR 0145 — deadline check before scan.
                        if let Some(b) = self.budget.as_ref() { b.check_deadline()?; }
                        let requested = if p.consume_symbol('*') { None } else {
                            let mut names = vec![p.word()?];
                            while p.consume_symbol(',') { names.push(p.word()?); }
                            Some(names)
                        };
                        p.expect("from")?;
                        let table = self.engine.resolve(&p.name()?)?;
                        let columns = requested.unwrap_or_else(||
                            table.columns.iter().map(|c|c.name.clone()).collect());
                        let positions = columns.iter()
                            .map(|name| locate(&table, name))
                            .collect::<SqlResult<Vec<_>>>()?;
                        let rows: Vec<Vec<_>> = if p.next_is_word("where") {
                            let key = key_predicate(&mut p, &table)?;
                            tx.get(table.id, &key)?.into_iter().collect()
                        } else {
                            tx.scan(table.id)?
                        };
                        p.end()?;
                        // ADR 0145 — charge rows after scan.
                        // ADR 0148 — charge materialized bytes.
                        if let Some(b) = self.budget.as_mut() {
                            b.charge_rows(rows.len() as u64)?;
                            b.charge_bytes(crate::query_budget::rows_bytes(&rows))?;
                            b.check_deadline()?;
                        }
                        Ok(SqlResultSet::Rows {
                            columns,
                            rows: rows.into_iter().map(|row: Vec<_>|
                                positions.iter().map(|pos|row[*pos].clone()).collect()
                            ).collect(),
                        })
                    }
                    "update" => {
                        // ADR 0147 — deadline check before mutation.
                        if let Some(b) = self.budget.as_ref() { b.check_deadline()?; }
                        let table = self.engine.resolve(&p.name()?)?;
                        p.expect("set")?;
                        let updates = p.assignments()?;
                        let key = key_predicate(&mut p, &table)?;
                        p.end()?;
                        let mut replacements = BTreeMap::new();
                        for (name, lit) in updates {
                            if table.primary_key.columns.contains(&name) {
                                return Err(SqlError::Unsupported("primary key UPDATE not supported".into()));
                            }
                            let position = locate(&table, &name)?;
                            if replacements.insert(position, coerce(&lit, &table.columns[position])?).is_some() {
                                return Err(SqlError::Syntax("duplicate SET column".into()));
                            }
                        }
                        let affected = if let Some(mut row) = tx.get(table.id, &key)? {
                            for (position, value) in replacements { row[position] = value; }
                            tx.replace(table.id, &row)?;
                            1
                        } else { 0 };
                        if let Some(b) = self.budget.as_mut() { b.charge_rows(affected)?; }
                        Ok(SqlResultSet::Command { affected })
                    }
                    "delete" => {
                        // ADR 0147 — deadline check before mutation.
                        if let Some(b) = self.budget.as_ref() { b.check_deadline()?; }
                        p.expect("from")?;
                        let table = self.engine.resolve(&p.name()?)?;
                        let key = key_predicate(&mut p, &table)?;
                        p.end()?;
                        let affected = if tx.get(table.id, &key)?.is_some() {
                            tx.delete(table.id, &key)?;
                            1
                        } else { 0 };
                        if let Some(b) = self.budget.as_mut() { b.charge_rows(affected)?; }
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
