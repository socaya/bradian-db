//! G23 opt-in SQL engine for G22 durable typed historical versions.
//! SELECT ... FROM db.schema.table AS OF <WAL_LSN> [WHERE full_pk].
//! History-domain format is separate from G15 shared-domain storage.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::{Column, Table};
use crate::runtime_limits::{ScanControl, HistoryLimits, HistoryUsage, StagingLimits, StagingUsage, TransactionLimits};
use crate::schema_activation::SchemaActivationReceipt;
use crate::persistent_catalog::PersistentCatalog;
use crate::sql::{
    coerce, key_predicate, locate, qualified, Literal, Parser, SqlError, SqlResult, SqlResultSet,
};
use crate::durable_typed::{DurableTypedStore, DurableTypedTransaction};

/// SQL-enabled shared transaction domain. The durable catalog and the domain
/// own separate locks and WALs. Row sessions never change metadata; exclusive
/// G36 DDL coordinates catalog/history recovery outside row transactions.
pub struct HistorySqlEngine {
    catalog: PersistentCatalog,
    domain: DurableTypedStore,
    _root: PathBuf,
    failed_ddl: bool,
}

impl HistorySqlEngine {
    pub fn open(root: impl AsRef<Path>) -> SqlResult<Self> {
        Self::open_with_history_limits(root, HistoryLimits::default())
    }

    pub fn open_with_history_limits(root: impl AsRef<Path>, limits: HistoryLimits) -> SqlResult<Self> {
        Self::open_with_resource_limits(root,limits,TransactionLimits::default())
    }

    pub fn open_with_resource_limits(root:impl AsRef<Path>, limits:HistoryLimits, transactions:TransactionLimits) -> SqlResult<Self> {
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
        let domain = DurableTypedStore::open_coordinated(root.join("history-domain"), &catalog, limits, transactions)?;
        domain.verify_ddl_catalog_horizon(catalog.last_lsn())?;
        Ok(Self { catalog, domain, _root: root.to_owned(), failed_ddl: false })
    }

    fn resolve(&self, parts: &[String]) -> SqlResult<Table> {
        self.require_ready()?;
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

    fn require_ready(&self) -> SqlResult<()> {
        if self.failed_ddl {
            return Err(SqlError::Unsupported("schema activation was interrupted; reopen for recovery".into()));
        }
        Ok(())
    }

    /// Explicitly retire inactive G28-G31 work without applying any proposal.
    pub fn retire_legacy_schema_intents(&mut self) -> SqlResult<crate::schema_activation::LegacyDdlRetirementReceipt> {
        self.require_ready()?;
        Ok(self.domain.retire_legacy_schema_intents(&self.catalog)?)
    }

    pub fn legacy_schema_intents_retired(&self) -> bool { self.domain.legacy_schema_intents_retired() }

    /// Exclusive, recoverable nullable-column addition. Row sessions cannot overlap
    /// a mutable engine borrow. Returns the durable history/schema receipt.
    pub fn add_nullable_column(
        &mut self, qualified_table: &str, column: Column,
    ) -> SqlResult<SchemaActivationReceipt> {
        let mut parser = Parser::new(qualified_table)?;
        let parts = parser.name()?;
        parser.end()?;
        let mut proposed = self.resolve(&parts)?;
        proposed.columns.push(column);
        self.activate_schema(proposed, &mut |_| Ok(()))
    }

    /// Append a column with an immutable typed default; old rows project that value.
    pub fn add_column_with_default(
        &mut self, qualified_table: &str, column: Column, default: crate::typed_table::Value,
    ) -> SqlResult<SchemaActivationReceipt> {
        let mut parser = Parser::new(qualified_table)?;
        let parts = parser.name()?;
        parser.end()?;
        let mut proposed = self.resolve(&parts)?;
        proposed.column_defaults.insert(column.name.clone(), default);
        proposed.columns.push(column);
        self.activate_schema(proposed, &mut |_| Ok(()))
    }

    /// Relax an existing non-key NOT NULL column through the same durable protocol.
    pub fn drop_not_null(
        &mut self, qualified_table: &str, column_name: &str,
    ) -> SqlResult<SchemaActivationReceipt> {
        let mut parser = Parser::new(qualified_table)?;
        let parts = parser.name()?;
        parser.end()?;
        let mut proposed = self.resolve(&parts)?;
        let mut column = Parser::new(column_name)?;
        let name = column.word()?;
        column.end()?;
        let index = locate(&proposed, &name)?;
        proposed.columns[index].nullable = true;
        self.activate_schema(proposed, &mut |_| Ok(()))
    }

    pub(crate) fn activate_schema(
        &mut self, proposed: Table,
        hook: &mut impl FnMut(crate::schema_activation::ActivationStage) -> Result<(), crate::transaction_domain::DomainError>,
    ) -> SqlResult<SchemaActivationReceipt> {
        self.require_ready()?;
        let intent = self.domain.plan_schema_activation(&self.catalog, proposed)?;
        self.failed_ddl = true;
        let result = self.domain.publish_schema_activation(&self.catalog, intent, hook);
        if result.is_ok() { self.failed_ddl = false; }
        Ok(result?)
    }

    /// Exclusive ALTER supports ADD [COLUMN] name type [NULL] and
    /// ALTER [COLUMN] name DROP NOT NULL. Other clauses fail before journaling.
    pub fn execute_ddl(&mut self, sql: &str) -> SqlResult<SchemaActivationReceipt> {
        self.require_ready()?;
        let mut p = Parser::new(sql)?;
        p.expect("alter")?;
        p.expect("table")?;
        let parts = p.name()?;
        qualified(&parts, 3)?;
        if p.consume_word("add") {
            p.consume_word("column");
            let name = p.word()?;
            let data_type = p.data_type()?;
            crate::catalog::validate_type(&data_type).map_err(|err| SqlError::Type(err.to_string()))?;
            let mut nullable = true;
            let mut null_clause = false;
            let mut default = None;
            loop {
                if p.consume_word("default") {
                    if default.is_some() { return Err(SqlError::Syntax("duplicate DEFAULT clause".into())); }
                    default = Some(p.literal()?);
                } else if p.consume_word("not") {
                    if null_clause { return Err(SqlError::Syntax("duplicate nullability clause".into())); }
                    p.expect("null")?;
                    null_clause = true;
                    nullable = false;
                } else if p.consume_word("null") {
                    if null_clause { return Err(SqlError::Syntax("duplicate nullability clause".into())); }
                    null_clause = true;
                } else { break; }
            }
            p.end()?;
            let column = Column { name, data_type, nullable };
            match default {
                Some(literal) => {
                    let value = coerce(&literal, &column)?;
                    self.add_column_with_default(&parts.join("."), column, value)
                },
                None => self.add_nullable_column(&parts.join("."), column),
            }
        } else {
            p.expect("alter")?;
            p.consume_word("column");
            let name = p.word()?;
            p.expect("drop")?;
            p.expect("not")?;
            p.expect("null")?;
            p.end()?;
            self.drop_not_null(&parts.join("."), &name)
        }
    }

    /// Only independently provisioned durable schema pins may be used.
    pub fn registered_table_ids(&self) -> Vec<crate::TableId> {
        self.domain.active_tables()
    }

    /// Observe the in-memory snapshot retention state.
    pub fn history_usage(&self) -> HistoryUsage { self.domain.history_usage() }
    pub fn history_limits(&self) -> HistoryLimits { self.domain.history_limits() }
    pub fn transaction_limits(&self) -> TransactionLimits { self.domain.transaction_limits() }

    pub fn mvcc_stats(&self) -> crate::durable_mvcc::DurableMvccStats {
        self.domain.stats()
    }

    pub fn last_lsn(&self) -> u64 { self.domain.last_lsn() }
    pub fn retention_floor(&self) -> u64 { self.domain.retention_floor() }

    /// Original and activated immutable schema epochs, recovered from history.
    /// A new epoch does not rewrite older schema definitions.
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

    /// G31: certify a prepared and coordinated proposal as READY FOR REVIEW.
    /// This writes only an inactive historical WAL record, not ALTER TABLE.
    pub fn certify_schema_readiness(&self, qualified_table: &str) -> SqlResult<u64> {
        let mut parser = Parser::new(qualified_table)?;
        let parts = parser.name()?;
        parser.end()?;
        let table = self.resolve(&parts)?;
        Ok(self.domain.certify_schema_readiness(table.id, self.catalog.last_lsn())?)
    }

    pub fn ddl_readiness_records(
        &self,
    ) -> Vec<(crate::durable_typed::DdlReadinessRecord, u64)> {
        self.domain.ddl_readiness_records()
    }

    /// Opt in to G35 BRVE writes in subsequently started transactions.
    /// Existing data and schemas are not rewritten; this is not ALTER TABLE.
    pub fn enable_epoch_row_writes(&self) -> SqlResult<u64> {
        self.require_ready()?;
        Ok(self.domain.enable_epoch_row_writes()?)
    }

    pub fn epoch_row_writes_enabled(&self) -> SqlResult<bool> {
        Ok(self.domain.epoch_row_writes_enabled()?)
    }

    pub fn checkpoint(&self) -> SqlResult<u64> { self.require_ready()?; Ok(self.domain.checkpoint()?) }

    /// Create a separate verified G24 archive while both source storage locks
    /// remain owned by this engine. G19 legacy archives are not compatible.
    pub fn backup_to(
        &self, destination: impl AsRef<Path>,
    ) -> crate::history_backup::HistoryBackupResult<crate::history_backup::HistoryBackupReport> {
        self.backup_to_with_metadata_limits(destination,crate::runtime_limits::MetadataLimits::default())
    }

    pub fn backup_to_with_metadata_limits(
        &self, destination: impl AsRef<Path>, metadata: crate::runtime_limits::MetadataLimits,
    ) -> crate::history_backup::HistoryBackupResult<crate::history_backup::HistoryBackupReport> {
        if self.failed_ddl {
            return Err(crate::history_backup::HistoryBackupError::Invalid("schema activation requires recovery before backup".into()));
        }
        crate::history_backup::backup_history_database(
            &self._root, destination.as_ref(), &self.catalog, &self.domain, metadata,
        )
    }

    /// Every callback owns a separate pinned MVCC snapshot. BEGIN
    /// opens a domain-wide row snapshot; COMMIT publishes one WAL frame.
    /// Leaving with a pending transaction discards all staged modifications.
    pub fn with_session<R>(
        &self,
        callback: impl FnOnce(&mut HistorySqlSession<'_>) -> SqlResult<R>,
    ) -> SqlResult<R> {
        self.with_session_control(ScanControl::default(), callback)
    }

    /// Explicit query controls; cloned handles can cancel scans from another thread.
    pub fn with_session_control<R>(
        &self, control: ScanControl,
        callback: impl FnOnce(&mut HistorySqlSession<'_>) -> SqlResult<R>,
    ) -> SqlResult<R> {
        self.with_session_limits(control, StagingLimits::default(), callback)
    }

    /// Separate query-buffer and transaction staging policies; neither bounds total RSS.
    pub fn with_session_limits<R>(
        &self, control: ScanControl, staging: StagingLimits,
        callback: impl FnOnce(&mut HistorySqlSession<'_>) -> SqlResult<R>,
    ) -> SqlResult<R> {
        self.require_ready()?;
        let mut session = HistorySqlSession {
            engine: self,
            control,
            staging,
            active: None,
            failed_statement: false,
            last_commit_lsn: None,
        };
        callback(&mut session)
    }
}

/// A scoped SQL session that may access several registered catalog tables.
pub struct HistorySqlSession<'a> {
    engine: &'a HistorySqlEngine,
    control: ScanControl,
    staging: StagingLimits,
    active: Option<DurableTypedTransaction<'a>>,
    last_commit_lsn: Option<u64>,
    failed_statement: bool,
}

impl<'a> HistorySqlSession<'a> {
    pub fn staging_usage(&self) -> Option<StagingUsage> { self.active.as_ref().map(DurableTypedTransaction::staging_usage) }
    pub fn in_transaction(&self) -> bool { self.active.is_some() }
    pub fn last_commit_lsn(&self) -> Option<u64> { self.last_commit_lsn }
    pub fn read_validation_enabled(&self) -> bool {
        self.active.as_ref().is_some_and(DurableTypedTransaction::read_validation_enabled)
    }
    pub fn snapshot_lsn(&self) -> Option<u64> {
        self.active.as_ref().map(DurableTypedTransaction::start_lsn)
    }

    /// Errors abort the transaction unless a savepoint permits explicit recovery.
    /// A failed transaction cannot commit before rollback-to or full rollback.
    pub fn execute(&mut self, sql: &str) -> SqlResult<SqlResultSet> {
        let result = self.execute_inner(sql);
        if result.is_err() {
            if self.active.as_ref().is_some_and(|tx| tx.savepoint_count() > 0) {
                self.failed_statement = true;
            } else {
                self.active = None;
                self.failed_statement = false;
            }
        }
        result
    }

    fn execute_inner(&mut self, sql: &str) -> SqlResult<SqlResultSet> {
        let mut p = Parser::new_expressions(sql)?;
        let command = p.word()?;
        if self.failed_statement && command != "rollback" {
            return Err(SqlError::Unsupported("transaction requires ROLLBACK TO a savepoint or ROLLBACK after a statement error".into()));
        }
        match command.as_str() {
            "begin" => {
                p.consume_word("transaction");
                let validate_reads = if p.consume_word("with") {
                    p.expect("read")?;
                    p.expect("validation")?;
                    true
                } else { false };
                p.end()?;
                if self.active.is_some() {
                    return Err(SqlError::Unsupported("nested BEGIN is unsupported".into()));
                }
                let tx: DurableTypedTransaction<'a> = self.engine.domain.try_begin_with_limits(self.staging, validate_reads)?;
                self.active = Some(tx);
                self.failed_statement = false;
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
            "savepoint" | "release" => {
                if command == "release" { p.consume_word("savepoint"); }
                let name = p.word()?;
                p.end()?;
                let tx = self.active.as_mut().ok_or_else(||
                    SqlError::Unsupported("savepoint operation requires BEGIN".into()))?;
                if command == "savepoint" { tx.savepoint(&name)?; }
                else { tx.release_savepoint(&name)?; }
                Ok(SqlResultSet::Command { affected: 0 })
            }
            "rollback" => {
                if p.consume_word("to") {
                    p.consume_word("savepoint");
                    let name = p.word()?;
                    p.end()?;
                    self.active.as_mut().ok_or_else(||
                        SqlError::Unsupported("ROLLBACK TO without BEGIN".into()))?.rollback_to(&name)?;
                    self.failed_statement = false;
                    return Ok(SqlResultSet::Command { affected: 0 });
                }
                p.end()?;
                let tx = self.active.take().ok_or_else(||
                    SqlError::Unsupported("ROLLBACK without BEGIN".into()))?;
                tx.rollback();
                self.failed_statement = false;
                Ok(SqlResultSet::Command { affected: 0 })
            }
            "describe" => {
                if self.read_validation_enabled() {
                    return Err(SqlError::Unsupported("historical DESCRIBE requires a separate snapshot session".into()));
                }
                // Read-only G26 inspection is available with or without BEGIN.
                // It does not stage metadata changes or start an MVCC writer.
                p.expect("table")?;
                let table = self.engine.resolve(&p.name()?)?;
                p.expect("as")?;
                p.expect("of")?;
                let lsn = p.number_u64()?;
                let defaults = if p.consume_word("with") { p.expect("defaults")?; true } else { false };
                p.end()?;
                let (schema, registered) = self.engine.domain.schema_as_of(table.id, lsn)?;
                let rows = schema.columns.iter().map(|column| {
                    let mut row = vec![
                        crate::typed_table::Value::String(column.name.clone()),
                        crate::typed_table::Value::String(serde_json::to_string(&column.data_type)
                            .expect("validated catalog data type is serializable")),
                        crate::typed_table::Value::Boolean(column.nullable),
                        crate::typed_table::Value::String(registered.to_string()),
                    ];
                    if defaults {
                        row.push(crate::typed_table::Value::Boolean(schema.column_defaults.contains_key(&column.name)));
                        row.push(schema.column_defaults.get(&column.name).cloned().unwrap_or(crate::typed_table::Value::Null));
                    }
                    row
                }).collect();
                let mut columns = vec!["column_name".into(), "data_type".into(),
                    "nullable".into(), "schema_registered_lsn".into()];
                if defaults { columns.extend(["has_default".into(), "column_default".into()]); }
                Ok(SqlResultSet::Rows { columns, rows })
            }
            "insert" | "select" | "update" | "delete" => {
                // Resolve and parse before applying any change. On any error,
                // execute() aborts or requires explicit savepoint recovery.
                let tx = self.active.as_mut().ok_or_else(||
                    SqlError::Unsupported("BEGIN required before transactional DML".into()))?;
                match command.as_str() {
                    "insert" => {
                        p.expect("into")?;
                        let table = self.engine.resolve(&p.name()?)?;
                        let names = p.list(|parser| parser.word())?;
                        p.expect("values")?;
                        let values = p.list(|parser| parser.insert_literal())?;
                        p.end()?;
                        if names.len() != values.len() {
                            return Err(SqlError::Syntax("INSERT column and value counts differ".into()));
                        }
                        let mut fields = BTreeMap::new();
                        for (name, value) in names.into_iter().zip(values) {
                            locate(&table, &name)?;
                            if fields.insert(name.clone(), value).is_some() {
                                return Err(SqlError::Syntax(format!("duplicate column {name}")));
                            }
                        }
                        let row = table.columns.iter().map(|column| {
                            match fields.remove(&column.name) {
                                Some(lit) if !matches!(lit, Literal::Default) => coerce(&lit, column),
                                _ => match table.column_defaults.get(&column.name) {
                                    Some(value) => Ok(value.clone()),
                                    None if column.nullable => Ok(crate::typed_table::Value::Null),
                                    None => Err(SqlError::Type(format!("missing NOT NULL column {}", column.name))),
                                },
                            }
                        }).collect::<SqlResult<Vec<_>>>()?;
                        tx.insert(table.id, &row)?;
                        Ok(SqlResultSet::Command { affected: 1 })
                    }
                    "select" => {
                        self.control.check()?;
                        let requested = if p.consume_symbol('*') { None } else {
                            let mut names = vec![p.word()?];
                            while p.consume_symbol(',') { names.push(p.word()?); }
                            Some(names)
                        };
                        p.expect("from")?;
                        let table = self.engine.resolve(&p.name()?)?;
                        // Historical SELECT is read-only: it may run inside
                        // a normal BEGIN session but cannot stage mutations at
                        // a past LSN. Queries below the retention floor fail.
                        let as_of = if p.consume_word("as") {
                            p.expect("of")?;
                            Some(p.number_u64()?)
                        } else { None };
                        if as_of.is_some() && tx.read_validation_enabled() {
                            return Err(SqlError::Unsupported("AS OF requires a separate snapshot session when read validation is enabled".into()));
                        }
                        let table = match as_of {
                            Some(lsn) => self.engine.domain.schema_as_of(table.id, lsn)?.0,
                            None => table,
                        };
                        let columns = requested.unwrap_or_else(||
                            table.columns.iter().map(|c|c.name.clone()).collect());
                        let positions = columns.iter()
                            .map(|name| locate(&table, name))
                            .collect::<SqlResult<Vec<_>>>()?;
                        let key = if p.next_is_word("where") { Some(key_predicate(&mut p, &table)?) } else { None };
                        let window = crate::select_window::SelectWindow::parse(&mut p, &table)?;
                        p.end()?;
                        let control = match window.input_row_limit() {
                            Some(limit) => self.control.cap_rows(limit),
                            None => self.control.clone(),
                        };
                        let rows = if let Some(key) = key {
                            match as_of {
                                Some(lsn) => self.engine.domain.get_as_of(table.id, &key, lsn)?
                                    .into_iter().collect(),
                                None => tx.get(table.id, &key)?.into_iter().collect(),
                            }
                        } else {
                            match as_of {
                                Some(lsn) => self.engine.domain.scan_as_of_with_control(table.id, lsn, &control)?,
                                None => tx.scan_with_control(table.id, &control)?,
                            }
                        };
                        control.check()?;
                        let rows = window.apply(rows, &table)?;
                        control.check()?;
                        Ok(SqlResultSet::Rows {
                            columns,
                            rows: crate::typed_table::project_rows_with_control(rows, &positions, &control)?,
                        })
                    }
                    "update" => {
                        let table = self.engine.resolve(&p.name()?)?;
                        p.expect("set")?;
                        let updates = crate::sql_expression::assignments(&mut p)?;
                        let key = key_predicate(&mut p, &table)?;
                        p.end()?;
                        let mut replacements = BTreeMap::new();
                        for (name, expression) in updates {
                            if table.primary_key.columns.contains(&name) {
                                return Err(SqlError::Unsupported("primary key UPDATE not supported".into()));
                            }
                            let position = locate(&table, &name)?;
                            if replacements.insert(position, crate::sql_expression::bind(&table, position, expression)?).is_some() {
                                return Err(SqlError::Syntax("duplicate SET column".into()));
                            }
                        }
                        let affected = if let Some(mut row) = tx.get(table.id, &key)? {
                            let original = row.clone();
                            for (position, expression) in replacements {
                                row[position] = expression.evaluate(&original, &table.columns[position])?;
                            }
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
