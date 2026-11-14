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
use crate::sql::{
    coerce, key_predicate, locate, qualified, Parser, SqlError, SqlResult, SqlResultSet,
};
use crate::typed_mvcc::{TypedMvccStore, TypedMvccTransaction};

/// SQL-enabled shared transaction domain. The durable catalog and the domain
/// own separate locks and WALs: existing metadata is resolved but not changed
/// by a domain transaction. DDL is intentionally unavailable in this API.
pub struct MvccSqlEngine {
    catalog: PersistentCatalog,
    domain: TypedMvccStore,
    _root: PathBuf,
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
        if root.as_ref().join("history-domain").exists() {
            return Err(SqlError::Unsupported(
                "G23 durable history root requires HistorySqlEngine; legacy SQL ownership refused".into()
            ));
        }
        let root = root.as_ref();
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
            crate::lifecycle::CatalogLifecycle::open(root)
                .map_err(|e| SqlError::Unsupported(format!(
                    "G18 lifecycle reconciliation failed: {e}"
                )))?;
        }
        let catalog = PersistentCatalog::open_with_limits(root.join("catalog"), catalog_limits)?;
        let definitions = catalog.snapshot().tables()
            .map(|table| (table.id, table.clone()))
            .collect::<BTreeMap<_,_>>();
        let domain = TypedMvccStore::open_with_limits(
            root.join("shared-domain"), &definitions, mvcc_limits,
        )?;
        Ok(Self { catalog, domain, _root: root.to_owned() })
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
        };
        callback(&mut session)
    }
}

/// A scoped SQL session that may access several registered catalog tables.
pub struct MvccSqlSession<'a> {
    engine: &'a MvccSqlEngine,
    active: Option<TypedMvccTransaction<'a>>,
    last_commit_lsn: Option<u64>,
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
        let mut p = Parser::new(sql)?;
        let command = p.word()?;
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
                        Ok(SqlResultSet::Command { affected: 1 })
                    }
                    "select" => {
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
                        let rows = if p.next_is_word("where") {
                            let key = key_predicate(&mut p, &table)?;
                            tx.get(table.id, &key)?.into_iter().collect()
                        } else {
                            tx.scan(table.id)?
                        };
                        p.end()?;
                        Ok(SqlResultSet::Rows {
                            columns,
                            rows: rows.into_iter().map(|row: Vec<_>|
                                positions.iter().map(|pos|row[*pos].clone()).collect()
                            ).collect(),
                        })
                    }
                    "update" => {
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
                        Ok(SqlResultSet::Command { affected })
                    }
                    "delete" => {
                        p.expect("from")?;
                        let table = self.engine.resolve(&p.name()?)?;
                        let key = key_predicate(&mut p, &table)?;
                        p.end()?;
                        let affected = if tx.get(table.id, &key)?.is_some() {
                            tx.delete(table.id, &key)?;
                            1
                        } else { 0 };
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
