//! G23 opt-in SQL engine for G22 durable typed historical versions.
//! SELECT ... FROM db.schema.table AS OF <WAL_LSN> [WHERE full_pk].
//! History-domain format is separate from G15 shared-domain storage.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::Table;
use crate::persistent_catalog::PersistentCatalog;
use crate::sql::{
    coerce, key_predicate, locate, qualified, Parser, SqlError, SqlResult, SqlResultSet,
};
use crate::durable_typed::{DurableTypedStore, DurableTypedTransaction};

/// SQL-enabled shared transaction domain. The durable catalog and the domain
/// own separate locks and WALs: existing metadata is resolved but not changed
/// by a domain transaction. DDL is intentionally unavailable in this API.
pub struct HistorySqlEngine {
    catalog: PersistentCatalog,
    domain: DurableTypedStore,
    _root: PathBuf,
}

impl HistorySqlEngine {
    pub fn open(root: impl AsRef<Path>) -> SqlResult<Self> {
        let root = root.as_ref();
        // Refuse opening a root with ANY legacy per-table data. No implicit
        // migration, dual writes, or ambiguous source of truth is allowed.
        let old_tables = root.join("tables");
        if old_tables.exists() && fs::read_dir(&old_tables)?.next().is_some() {
            return Err(SqlError::Unsupported(
                "legacy per-table storage exists; explicit verified migration is required".into(),
            ));
        }
        // Opening history SQL must not create a new database implicitly.
        if !root.is_dir() { return Err(SqlError::Unsupported("history database root missing".into())); }
        // G23 cannot provision missing tables. Opening a root that has
        // never been registered must fail rather than silently start a new
        // physical storage domain with zero data.
        if !root.join("catalog").is_dir() || !root.join("history-domain").is_dir() {
            return Err(SqlError::Unsupported(
                "G23 requires an existing provisioned durable-history database".into(),
            ));
        }
        // This dedicated history root has independently provisioned immutable
        // schema pins. G18's older shared-domain lifecycle is not applicable.
        if root.join("lifecycle").exists() || root.join("shared-domain").exists() {
            return Err(SqlError::Unsupported(
                "G23 requires a separate history database root; G15/G18 data must be migrated".into(),
            ));
        }
        let catalog = PersistentCatalog::open(root.join("catalog"))?;
        let definitions = catalog.snapshot().tables()
            .map(|table| (table.id, table.clone()))
            .collect::<BTreeMap<_,_>>();
        let domain = DurableTypedStore::open(root.join("history-domain"), &definitions)?;
        domain.verify_ddl_catalog_horizon(catalog.last_lsn())?;
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
                "table {} has no matching G23 durable history-domain pin", table.id.0
            )));
        }
        Ok(table.clone())
    }

    /// Only independently provisioned durable schema pins may be used.
    pub fn registered_table_ids(&self) -> Vec<crate::TableId> {
        self.domain.active_tables()
    }

    /// Observe the in-memory snapshot retention state.
    pub fn mvcc_stats(&self) -> crate::durable_mvcc::DurableMvccStats {
        self.domain.stats()
    }

    pub fn last_lsn(&self) -> u64 { self.domain.last_lsn() }
    pub fn retention_floor(&self) -> u64 { self.domain.retention_floor() }

    /// G26: immutable, validated schema registration epochs for this history
    /// database. Schema changes after registration remain unsupported.
    pub fn schema_epochs(&self) -> Vec<(crate::TableId, u64)> {
        self.domain.schema_epochs()
    }

    /// Read the schema applicable at a retained historical WAL position.
    /// The returned LSN is the original immutable schema registration commit.
    pub fn schema_as_of(&self, qualified_table: &str, lsn: u64) -> SqlResult<(Table, u64)> {
        let mut parser = Parser::new(qualified_table)?;
        let parts = parser.name()?;
        parser.end()?;
        let table = self.resolve(&parts)?;
        Ok(self.domain.schema_as_of(table.id, lsn)?)
    }
    /// G27 preview: decode retained rows with the ORIGINAL immutable schema
    /// and present them under a proposed append-only nullable-column shape.
    /// No catalog or historical WAL changes are made by this method.
    pub fn preview_nullable_additions_as_of(
        &self,
        qualified_table: &str,
        lsn: u64,
        proposed: &Table,
    ) -> SqlResult<Vec<Vec<crate::typed_table::Value>>> {
        let mut parser = Parser::new(qualified_table)?;
        let parts = parser.name()?;
        parser.end()?;
        let current = self.resolve(&parts)?;
        let (original, _) = self.domain.schema_as_of(current.id, lsn)?;
        let plan = crate::schema_evolution::plan_nullable_additions(&original, proposed)?;
        let rows = self.domain.scan_as_of(original.id, lsn)?;
        rows.iter().map(|row| {
            crate::schema_evolution::project_row(&plan, row).map_err(SqlError::from)
        }).collect()
    }

    /// G28: persist a validated, inactive schema proposal in the historical
    /// WAL. This does not alter the live G11 catalog or allow ALTER TABLE.
    /// Callers must release the owner and reopen to inspect recovered intents.
    pub fn prepare_nullable_addition(
        &self, qualified_table: &str, proposed: &Table,
    ) -> SqlResult<u64> {
        let mut parser = Parser::new(qualified_table)?;
        let parts = parser.name()?;
        parser.end()?;
        let original = self.resolve(&parts)?;
        Ok(self.domain.prepare_nullable_addition(original.id, proposed)?)
    }

    pub fn prepared_schema_proposals(&self) -> Vec<(crate::TableId, Table, u64)> {
        self.domain.prepared_schema_proposals()
    }

    /// G29: permanently retire a prepared schema change with a synchronized
    /// historical WAL marker. This does not update the live catalog.
    pub fn abort_prepared_schema(&self, qualified_table: &str) -> SqlResult<u64> {
        let mut parser = Parser::new(qualified_table)?;
        let parts = parser.name()?;
        parser.end()?;
        let table = self.resolve(&parts)?;
        Ok(self.domain.abort_prepared_schema(table.id)?)
    }

    /// Returns WAL-recovered abort markers at engine open. Reopen to refresh.
    pub fn aborted_schema_proposals(&self) -> Vec<(crate::TableId, u64)> {
        self.domain.aborted_schema_proposals()
    }

    /// G30: bind an inactive G28 proposal to the authoritative G11 catalog
    /// WAL position. Both stores remain exclusively held by this engine.
    pub fn coordinate_prepared_schema(&self, qualified_table: &str) -> SqlResult<u64> {
        let mut parser = Parser::new(qualified_table)?;
        let name = parser.name()?;
        parser.end()?;
        let table = self.resolve(&name)?;
        Ok(self.domain.coordinate_prepared_schema(table.id, self.catalog.last_lsn())?)
    }

    /// Recovered coordination records from this engine's opening snapshot.
    pub fn ddl_coordination_records(
        &self,
    ) -> Vec<(crate::durable_typed::DdlCoordinationRecord, u64)> {
        self.domain.ddl_coordination_records()
    }

    pub fn checkpoint(&self) -> SqlResult<u64> { Ok(self.domain.checkpoint()?) }

    /// Create a separate verified G24 archive while both source storage locks
    /// remain owned by this engine. G19 legacy archives are not compatible.
    pub fn backup_to(
        &self, destination: impl AsRef<Path>,
    ) -> crate::history_backup::HistoryBackupResult<crate::history_backup::HistoryBackupReport> {
        crate::history_backup::backup_history_database(
            &self._root, destination.as_ref(), &self.catalog, &self.domain,
        )
    }

    /// Every callback owns a separate pinned MVCC snapshot. BEGIN
    /// opens a domain-wide row snapshot; COMMIT publishes one WAL frame.
    /// Leaving with a pending transaction discards all staged modifications.
    pub fn with_session<R>(
        &self,
        callback: impl FnOnce(&mut HistorySqlSession<'_>) -> SqlResult<R>,
    ) -> SqlResult<R> {
        let mut session = HistorySqlSession {
            engine: self,
            active: None,
            last_commit_lsn: None,
        };
        callback(&mut session)
    }
}

/// A scoped SQL session that may access several registered catalog tables.
pub struct HistorySqlSession<'a> {
    engine: &'a HistorySqlEngine,
    active: Option<DurableTypedTransaction<'a>>,
    last_commit_lsn: Option<u64>,
}

impl<'a> HistorySqlSession<'a> {
    pub fn in_transaction(&self) -> bool { self.active.is_some() }
    pub fn last_commit_lsn(&self) -> Option<u64> { self.last_commit_lsn }
    pub fn snapshot_lsn(&self) -> Option<u64> {
        self.active.as_ref().map(DurableTypedTransaction::start_lsn)
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
                let tx: DurableTypedTransaction<'a> = self.engine.domain.begin();
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
            "describe" => {
                // Read-only G26 inspection is available with or without BEGIN.
                // It does not stage metadata changes or start an MVCC writer.
                p.expect("table")?;
                let table = self.engine.resolve(&p.name()?)?;
                p.expect("as")?;
                p.expect("of")?;
                let lsn = p.number_u64()?;
                p.end()?;
                let (schema, registered) = self.engine.domain.schema_as_of(table.id, lsn)?;
                let rows = schema.columns.into_iter().map(|column| vec![
                    crate::typed_table::Value::String(column.name),
                    crate::typed_table::Value::String(
                        serde_json::to_string(&column.data_type)
                            .expect("validated catalog data type is serializable")
                    ),
                    crate::typed_table::Value::Boolean(column.nullable),
                    crate::typed_table::Value::String(registered.to_string()),
                ]).collect();
                Ok(SqlResultSet::Rows {
                    columns: vec![
                        "column_name".into(), "data_type".into(),
                        "nullable".into(), "schema_registered_lsn".into(),
                    ],
                    rows,
                })
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
                        // Historical SELECT is read-only: it may run inside
                        // a normal BEGIN session but cannot stage mutations at
                        // a past LSN. Queries below the retention floor fail.
                        let as_of = if p.consume_word("as") {
                            p.expect("of")?;
                            Some(p.number_u64()?)
                        } else { None };
                        let rows = if p.next_is_word("where") {
                            let key = key_predicate(&mut p, &table)?;
                            match as_of {
                                Some(lsn) => self.engine.domain.get_as_of(table.id, &key, lsn)?
                                    .into_iter().collect(),
                                None => tx.get(table.id, &key)?.into_iter().collect(),
                            }
                        } else {
                            match as_of {
                                Some(lsn) => self.engine.domain.scan_as_of(table.id, lsn)?,
                                None => tx.scan(table.id)?,
                            }
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
                "history-domain SQL sessions do not support {command}"
            ))),
        }
    }
}
