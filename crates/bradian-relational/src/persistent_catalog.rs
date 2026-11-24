//! WAL-backed, replay-validated Bradian relational metadata catalog.
//!
//! A catalog change is first applied to a private clone of the validated
//! in-memory Catalog, then durably appended as ONE BRWL mutation. Only after
//! WAL sync succeeds does that new metadata become visible. On restart, every
//! DDL operation is replayed through the original Catalog validators.
//!
//! This is an operational local transactional *metadata* journal, not SQL
//! parsing, schema migration of existing rows, MVCC, or multi-table data DDL.
//! The journal is append-only until a future versioned catalog compactor is
//! designed. Its physical store is exclusively owned by one RowStore process.

use std::path::Path;
use std::collections::BTreeMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    Catalog, CatalogError, DatabaseId, ForeignKeySpec, SchemaId, Table, TableId, TableSpec,
};
use crate::row_store::{BaceWiring, RowStore, StoreError};
use crate::runtime_limits::{CatalogLimits, CatalogUsage, ResourceError, StorageLimits};
use crate::backup::BackupReceipt;

const MARKER_KEY: &[u8] = b"bradian.catalog.version";
const MARKER_VALUE: &[u8] = b"BRADIAN-CATALOG-DDL-V1";
const EVENT_PREFIX: &str = "ddl/";
const JOURNAL_VERSION: u8 = 1;

#[derive(Debug, Error)]
pub enum PersistentCatalogError {
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error(transparent)]
    Storage(#[from] StoreError),
    #[error(transparent)]
    Encoding(#[from] serde_json::Error),
    #[error(transparent)]
    Resource(#[from] ResourceError),
    #[error("catalog journal is corrupt: {0}")]
    Corruption(String),
}

pub type PersistentCatalogResult<T> = Result<T, PersistentCatalogError>;

/// Exact proposal identity, including its journal position to prevent stale decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogTableRevision {
    pub proposal_lsn: u64,
    pub proposed: Table,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Command {
    CreateDatabase { name: String },
    CreateSchema { database: DatabaseId, name: String },
    CreateTable { schema: SchemaId, specification: TableSpec },
    AddForeignKey { table: TableId, specification: ForeignKeySpec },
    DropTable { table: TableId },
    DropSchema { schema: SchemaId },
    DropDatabase { database: DatabaseId },
    ProposeTableRevision { table: TableId, proposed: Table },
    ProposeNullableRevision { table: TableId, proposed: Table },
    ProposeDefaultedRevision { table: TableId, proposed: Table },
    ActivateTableRevision { revision: CatalogTableRevision },
    CancelTableRevision { revision: CatalogTableRevision },
    /// ADR 0165 — direct catalog ALTER (RENAME / DROP /
    /// SET DEFAULT / DROP DEFAULT / WIDEN). Journaled for
    /// durable replay; each kind is applied through the
    /// matching `Catalog::*` mutator and so inherits its
    /// validation + dependency checks.
    RenameTable { table: TableId, new_name: String },
    RenameColumn { table: TableId, old: String, new: String },
    DropColumn { table: TableId, column: String },
    SetColumnDefault { table: TableId, column: String, value: crate::typed_table::Value },
    DropColumnDefault { table: TableId, column: String },
    WidenColumnType { table: TableId, column: String, new_type: crate::model::DataType },
    /// ADR 0241 — SET NOT NULL. Flips the column's nullable
    /// bit after the SQL engine has validated that no existing
    /// row carries NULL in the column.
    StrengthenColumnNotNull { table: TableId, column: String },
    /// ADR 0259 — SET DEFAULT <expression>.
    SetColumnDefaultExpr { table: TableId, column: String, expr: crate::model::DefaultExpr },
    /// ADR 0259 — DROP expression default.
    DropColumnDefaultExpr { table: TableId, column: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Event {
    version: u8,
    /// Expected allocated catalog ID for CREATE, or None for other DDL.
    /// Replaying a different ID is an integrity failure, not a migration.
    result_id: Option<u64>,
    command: Command,
}

impl Event {
    fn apply(&self, catalog: &mut Catalog, revisions: &mut BTreeMap<TableId, CatalogTableRevision>, event_lsn: u64) -> PersistentCatalogResult<Option<u64>> {
        let result = match &self.command {
            Command::CreateDatabase { name } => {
                Some(catalog.create_database(name)?.0)
            }
            Command::CreateSchema { database, name } => {
                Some(catalog.create_schema(*database, name)?.0)
            }
            Command::CreateTable { schema, specification } => {
                Some(catalog.create_table(*schema, specification.clone())?.0)
            }
            Command::AddForeignKey { table, specification } => {
                if revisions.contains_key(table) {
                    return Err(PersistentCatalogError::Corruption("table has pending schema revision".into()));
                }
                catalog.add_foreign_key(*table, specification.clone())?;
                None
            }
            Command::DropTable { table } => {
                if revisions.contains_key(table) {
                    return Err(PersistentCatalogError::Corruption("table has pending schema revision".into()));
                }
                catalog.drop_table(*table)?;
                None
            }
            Command::DropSchema { schema } => {
                catalog.drop_schema(*schema)?;
                None
            }
            Command::DropDatabase { database } => {
                catalog.drop_database(*database)?;
                None
            }
            Command::ActivateTableRevision { revision } | Command::CancelTableRevision { revision } => {
                let proposed = &revision.proposed;
                if revisions.get(&proposed.id) != Some(revision) {
                    return Err(PersistentCatalogError::Corruption("revision decision does not match pending proposal".into()));
                }
                if matches!(&self.command, Command::ActivateTableRevision { .. }) {
                    catalog.activate_nullable_revision(proposed)?;
                }
                revisions.remove(&proposed.id);
                None
            }
            Command::RenameTable { table, new_name } => {
                catalog.rename_table(*table, new_name)?;
                None
            }
            Command::RenameColumn { table, old, new } => {
                catalog.rename_column(*table, old, new)?;
                None
            }
            Command::DropColumn { table, column } => {
                catalog.drop_column(*table, column)?;
                None
            }
            Command::SetColumnDefault { table, column, value } => {
                catalog.set_column_default(*table, column, value.clone())?;
                None
            }
            Command::DropColumnDefault { table, column } => {
                catalog.drop_column_default(*table, column)?;
                None
            }
            Command::WidenColumnType { table, column, new_type } => {
                catalog.widen_column_type(*table, column, new_type.clone())?;
                None
            }
            Command::StrengthenColumnNotNull { table, column } => {
                catalog.strengthen_column_not_null(*table, column)?;
                None
            }
            Command::SetColumnDefaultExpr { table, column, expr } => {
                catalog.set_column_default_expr(*table, column, expr.clone())?;
                None
            }
            Command::DropColumnDefaultExpr { table, column } => {
                catalog.drop_column_default_expr(*table, column)?;
                None
            }
            Command::ProposeTableRevision { table, proposed } | Command::ProposeNullableRevision { table, proposed } | Command::ProposeDefaultedRevision { table, proposed } => {
                let original = catalog.table(*table).ok_or_else(||
                    PersistentCatalogError::Corruption("revision refers to missing table".into())
                )?;
                if matches!(&self.command, Command::ProposeTableRevision { .. }) {
                    crate::schema_evolution::plan_nullable_additions(original, proposed)
                } else if matches!(&self.command, Command::ProposeNullableRevision { .. }) {
                    crate::schema_evolution::plan_nullable_evolution(original, proposed)
                } else { crate::schema_evolution::plan_default_additions(original, proposed) }
                    .map_err(|e| PersistentCatalogError::Corruption(e.to_string()))?;
                // Reuse the authoritative catalog validators without publishing.
                catalog.clone().activate_nullable_revision(proposed)?;
                if revisions.contains_key(table) {
                    return Err(PersistentCatalogError::Corruption("duplicate table revision".into()));
                }
                revisions.insert(*table, CatalogTableRevision { proposal_lsn: event_lsn, proposed: proposed.clone() });
                None
            }
        };
        if result != self.result_id {
            return Err(PersistentCatalogError::Corruption(
                "replayed identity differs from journaled identity".into(),
            ));
        }
        Ok(result)
    }
}

#[derive(Debug)]
struct Inner {
    catalog: Catalog,
    revisions: BTreeMap<TableId, CatalogTableRevision>,
    journal: RowStore,
}

fn usage_of(catalog: &Catalog, revisions: &BTreeMap<TableId, CatalogTableRevision>) -> CatalogUsage {
    CatalogUsage {
        databases: catalog.databases().count(),
        schemas: catalog.schemas().count(),
        tables: catalog.tables().count(),
        revisions: revisions.len(),
    }
}

/// One exclusively owned durable catalog. Metadata operations are serialized
/// under a process mutex, then WAL-synced before their new state is published.
#[derive(Debug)]
pub struct PersistentCatalog {
    inner: Mutex<Inner>,
    limits: CatalogLimits,
    /// ADR 0097 — reader-session pin registry. Maps revision
    /// (journal LSN at `reader_session()` time) to active pin
    /// count. Operator-visible via `active_reader_revisions()`.
    /// Non-blocking: DDL does not consult this map.
    pins: std::sync::Arc<Mutex<BTreeMap<u64, usize>>>,
}

/// ADR 0097 — RAII handle holding a Catalog snapshot at a
/// specific revision. The snapshot is immutable; DDL on the
/// live catalog does not mutate it. `Drop` deregisters the
/// pin so `PersistentCatalog::active_reader_revisions()`
/// reflects session lifetimes.
#[derive(Debug)]
pub struct ReaderSession {
    snapshot: Catalog,
    revision: u64,
    pins: std::sync::Arc<Mutex<BTreeMap<u64, usize>>>,
}

impl ReaderSession {
    pub fn revision(&self) -> u64 { self.revision }
    pub fn snapshot(&self) -> &Catalog { &self.snapshot }
}

impl Drop for ReaderSession {
    fn drop(&mut self) {
        let mut pins = self.pins.lock();
        if let Some(count) = pins.get_mut(&self.revision) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                pins.remove(&self.revision);
            }
        }
    }
}

impl PersistentCatalog {
    pub fn open(path: impl AsRef<Path>) -> PersistentCatalogResult<Self> {
        Self::open_with_limits(path, CatalogLimits::default())
    }

    /// Open the catalog with explicit decoded-state and event-byte admission.
    /// Policies are not persisted; they bound live decoded state only.
    pub fn open_with_limits(path: impl AsRef<Path>, limits: CatalogLimits)
        -> PersistentCatalogResult<Self>
    {
        Self::open_inner(path.as_ref(), limits, None)
    }

    /// ADR 0069 — open the catalog journal under BACE wiring.
    pub fn open_with_bace(
        path: impl AsRef<Path>,
        wiring: BaceWiring,
    ) -> PersistentCatalogResult<Self> {
        Self::open_with_bace_and_limits(path, wiring, CatalogLimits::default())
    }

    pub fn open_with_bace_and_limits(
        path: impl AsRef<Path>,
        wiring: BaceWiring,
        limits: CatalogLimits,
    ) -> PersistentCatalogResult<Self> {
        Self::open_inner(path.as_ref(), limits, Some(wiring))
    }

    fn open_inner(
        path: &Path,
        limits: CatalogLimits,
        wiring: Option<BaceWiring>,
    ) -> PersistentCatalogResult<Self> {
        let journal = match wiring {
            Some(w) => RowStore::open_with_bace(path, StorageLimits::default(), w)?,
            None    => RowStore::open(path)?,
        };
        let mut catalog = Catalog::new();
        let mut revisions = BTreeMap::new();
        let rows = journal.scan();
        if rows.is_empty() {
            journal.put(MARKER_KEY, MARKER_VALUE)?;
        } else {
            if journal.get(MARKER_KEY).as_deref() != Some(MARKER_VALUE) {
                return Err(PersistentCatalogError::Corruption(
                    "catalog version marker is absent or incompatible".into(),
                ));
            }
            // The marker is one WAL mutation; subsequent successful DDL
            // operations consume one sequential WAL LSN each.
            let mut next_lsn = 2_u64;
            for (key, value) in &rows {
                if key.as_slice() == MARKER_KEY {
                    continue;
                }
                let expected_key = event_key(next_lsn);
                if key != expected_key.as_bytes() {
                    return Err(PersistentCatalogError::Corruption(
                        format!("missing, duplicated, or unexpected DDL event at LSN {next_lsn}"),
                    ));
                }
                limits.check_event_bytes(value.len())?;
                let event: Event = serde_json::from_slice(value)
                    .map_err(|err| PersistentCatalogError::Corruption(
                        format!("invalid DDL event at LSN {next_lsn}: {err}"),
                    ))?;
                if event.version != JOURNAL_VERSION {
                    return Err(PersistentCatalogError::Corruption(
                        format!("unsupported DDL event version at LSN {next_lsn}"),
                    ));
                }
                event.apply(&mut catalog, &mut revisions, next_lsn)
                    .map_err(|err| PersistentCatalogError::Corruption(
                        format!("DDL replay failed at LSN {next_lsn}: {err}"),
                    ))?;
                // Bound decoded state after each event; refusal preserves the
                // on-disk journal exactly as it was and never silently prunes.
                limits.check(usage_of(&catalog, &revisions))?;
                next_lsn = next_lsn.checked_add(1)
                    .ok_or_else(|| PersistentCatalogError::Corruption(
                        "catalog event sequence overflow".into(),
                    ))?;
            }
            if journal.last_lsn() != next_lsn - 1 {
                return Err(PersistentCatalogError::Corruption(
                    "catalog journal LSN does not match event count".into(),
                ));
            }
        }
        Ok(Self {
            inner: Mutex::new(Inner { catalog, revisions, journal }),
            limits,
            pins: std::sync::Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    /// ADR 0097 — start a reader session at the current catalog
    /// revision. Returns an RAII handle whose Drop deregisters
    /// the pin. The snapshot is immutable; concurrent DDL
    /// creates new revisions but does NOT invalidate this view.
    pub fn reader_session(&self) -> ReaderSession {
        let inner = self.inner.lock();
        let revision = inner.journal.last_lsn();
        let snapshot = inner.catalog.clone();
        drop(inner);
        *self.pins.lock().entry(revision).or_insert(0) += 1;
        ReaderSession { snapshot, revision, pins: self.pins.clone() }
    }

    /// ADR 0097 — one-shot read of the pin registry. Operator
    /// diagnostic only; does NOT affect DDL. Returns map from
    /// revision → active pin count.
    pub fn active_reader_revisions(&self) -> BTreeMap<u64, usize> {
        self.pins.lock().clone()
    }

    pub fn resource_limits(&self) -> CatalogLimits { self.limits }
    pub fn resource_usage(&self) -> CatalogUsage {
        let inner = self.inner.lock();
        usage_of(&inner.catalog, &inner.revisions)
    }

    /// Return an owned consistent metadata view. A cloned catalog is necessary
    /// because future DDL must not mutate the reader's snapshot.
    pub fn snapshot(&self) -> Catalog {
        self.inner.lock().catalog.clone()
    }

    /// G33 proposals are durable, inactive and separate from the live catalog.
    pub fn proposed_table_revisions(&self) -> Vec<Table> {
        self.inner.lock().revisions.values().map(|revision| revision.proposed.clone()).collect()
    }

    pub fn proposed_table_revision(&self, id: TableId) -> Option<Table> {
        self.inner.lock().revisions.get(&id).map(|revision| revision.proposed.clone())
    }

    pub fn pending_table_revision(&self, id: TableId) -> Option<CatalogTableRevision> {
        self.inner.lock().revisions.get(&id).cloned()
    }

    pub fn last_lsn(&self) -> u64 {
        self.inner.lock().journal.last_lsn()
    }

    pub(crate) fn journal_row_count(&self) -> u64 {
        self.inner.lock().journal.len() as u64
    }

    /// Publish the catalog journal through Bradian's BRPG checkpoint protocol.
    /// The append-only logical DDL events themselves remain recoverable.
    pub fn checkpoint(&self) -> PersistentCatalogResult<u64> {
        let inner = self.inner.lock();
        Ok(inner.journal.checkpoint()?)
    }

    /// Export a consistent, independently verified backup of this catalog's
    /// metadata journal only. It does NOT include referenced user table files.
    /// The catalog mutex prevents any DDL while checkpointing and copying.
    pub fn backup_catalog_to(
        &self,
        destination: impl AsRef<Path>,
    ) -> PersistentCatalogResult<BackupReceipt> {
        let inner = self.inner.lock();
        Ok(inner.journal.backup_to(destination)?)
    }

    fn commit(&self, command: Command) -> PersistentCatalogResult<(Option<u64>, u64)> {
        let mut inner = self.inner.lock();
        let mut tentative = inner.catalog.clone();
        let mut pending = inner.revisions.clone();
        // Validate and allocate stable identities before modifying the WAL.
        let mut event = Event {
            version: JOURNAL_VERSION,
            result_id: None,
            command,
        };
        event.result_id = match &event.command {
            Command::CreateDatabase { name } => Some(tentative.create_database(name)?.0),
            Command::CreateSchema { database, name } => Some(tentative.create_schema(*database, name)?.0),
            Command::CreateTable { schema, specification } => Some(tentative.create_table(*schema, specification.clone())?.0),
            _ => None,
        };
        let next_lsn = inner.journal.last_lsn().checked_add(1)
            .ok_or(PersistentCatalogError::Storage(StoreError::SequenceExhausted))?;
        // Use exactly the replay validator for every operation before WAL publication.
        tentative = inner.catalog.clone();
        event.apply(&mut tentative, &mut pending, next_lsn)?;
        // Admit decoded state before serializing or journaling. A refusal leaves
        // the live catalog, revisions and WAL exactly as they were.
        self.limits.check(usage_of(&tentative, &pending))?;
        let bytes = serde_json::to_vec(&event)?;
        self.limits.check_event_bytes(bytes.len())?;
        // Oversized DDL definitions must fail before being committed. RowStore
        // enforces the inline record limit and leaves the WAL unmodified.
        let actual = inner.journal.put(event_key(next_lsn).as_bytes(), &bytes)?;
        if actual != next_lsn {
            return Err(PersistentCatalogError::Corruption(
                "catalog journal recorded unexpected LSN".into(),
            ));
        }
        inner.catalog = tentative;
        inner.revisions = pending;
        Ok((event.result_id, actual))
    }

    /// Append a replay-validated inactive schema proposal; NOT an ALTER.
    pub fn propose_table_revision(
        &self, table: TableId, proposed: Table,
    ) -> PersistentCatalogResult<u64> {
        Ok(self.commit(Command::ProposeTableRevision { table, proposed })?.1)
    }

    /// ADR 0165 — durable ALTER TABLE RENAME TO.
    pub fn rename_table(&self, table: TableId, new_name: &str) -> PersistentCatalogResult<u64> {
        Ok(self.commit(Command::RenameTable {
            table, new_name: new_name.into(),
        })?.1)
    }

    /// ADR 0165 — durable ALTER TABLE RENAME COLUMN.
    pub fn rename_column(&self, table: TableId, old: &str, new: &str)
        -> PersistentCatalogResult<u64>
    {
        Ok(self.commit(Command::RenameColumn {
            table, old: old.into(), new: new.into(),
        })?.1)
    }

    /// ADR 0165 — durable ALTER TABLE DROP COLUMN.
    pub fn drop_column(&self, table: TableId, column: &str) -> PersistentCatalogResult<u64> {
        Ok(self.commit(Command::DropColumn {
            table, column: column.into(),
        })?.1)
    }

    /// ADR 0166 — durable ALTER COLUMN SET DEFAULT.
    pub fn set_column_default(
        &self, table: TableId, column: &str,
        value: crate::typed_table::Value,
    ) -> PersistentCatalogResult<u64> {
        Ok(self.commit(Command::SetColumnDefault {
            table, column: column.into(), value,
        })?.1)
    }

    /// ADR 0166 — durable ALTER COLUMN DROP DEFAULT.
    pub fn drop_column_default(&self, table: TableId, column: &str)
        -> PersistentCatalogResult<u64>
    {
        Ok(self.commit(Command::DropColumnDefault {
            table, column: column.into(),
        })?.1)
    }

    /// ADR 0166 — durable ALTER COLUMN TYPE widen.
    pub fn widen_column_type(
        &self, table: TableId, column: &str,
        new_type: crate::model::DataType,
    ) -> PersistentCatalogResult<u64> {
        Ok(self.commit(Command::WidenColumnType {
            table, column: column.into(), new_type,
        })?.1)
    }

    /// ADR 0241 — SET NOT NULL. Caller must ensure no stored
    /// row carries NULL in this column before invoking.
    pub fn strengthen_column_not_null(
        &self, table: TableId, column: &str,
    ) -> PersistentCatalogResult<u64> {
        Ok(self.commit(Command::StrengthenColumnNotNull {
            table, column: column.into(),
        })?.1)
    }

    /// ADR 0259 — SET DEFAULT <expression>.
    pub fn set_column_default_expr(
        &self, table: TableId, column: &str, expr: crate::model::DefaultExpr,
    ) -> PersistentCatalogResult<u64> {
        Ok(self.commit(Command::SetColumnDefaultExpr {
            table, column: column.into(), expr,
        })?.1)
    }

    /// ADR 0259 — DROP expression default.
    pub fn drop_column_default_expr(
        &self, table: TableId, column: &str,
    ) -> PersistentCatalogResult<u64> {
        Ok(self.commit(Command::DropColumnDefaultExpr {
            table, column: column.into(),
        })?.1)
    }

    /// G39 proposal supporting non-key NOT NULL relaxation as well as nullable additions.
    pub(crate) fn propose_nullable_revision(
        &self, table: TableId, proposed: Table,
    ) -> PersistentCatalogResult<u64> {
        Ok(self.commit(Command::ProposeNullableRevision { table, proposed })?.1)
    }

    pub(crate) fn propose_defaulted_revision(
        &self, table: TableId, proposed: Table,
    ) -> PersistentCatalogResult<u64> {
        Ok(self.commit(Command::ProposeDefaultedRevision { table, proposed })?.1)
    }

    /// Activate catalog metadata only. Existing typed row domains require coordination.
    /// The exact expected definition prevents a stale caller deciding another proposal.
    pub fn activate_table_revision(&self, revision: CatalogTableRevision) -> PersistentCatalogResult<u64> {
        Ok(self.commit(Command::ActivateTableRevision { revision })?.1)
    }

    /// Durably discard the exact pending proposal without changing the live schema.
    pub fn cancel_table_revision(&self, revision: CatalogTableRevision) -> PersistentCatalogResult<u64> {
        Ok(self.commit(Command::CancelTableRevision { revision })?.1)
    }

    pub fn create_database(&self, name: &str) -> PersistentCatalogResult<DatabaseId> {
        Ok(DatabaseId(self.commit(Command::CreateDatabase { name: name.into() })?.0
            .expect("create allocates identity")))
    }

    pub fn create_schema(
        &self, database: DatabaseId, name: &str,
    ) -> PersistentCatalogResult<SchemaId> {
        Ok(SchemaId(self.commit(Command::CreateSchema { database, name: name.into() })?.0
            .expect("create allocates identity")))
    }

    pub fn create_table(
        &self, schema: SchemaId, specification: TableSpec,
    ) -> PersistentCatalogResult<TableId> {
        Ok(TableId(self.commit(Command::CreateTable { schema, specification })?.0
            .expect("create allocates identity")))
    }

    pub fn add_foreign_key(
        &self, table: TableId, specification: ForeignKeySpec,
    ) -> PersistentCatalogResult<()> {
        self.commit(Command::AddForeignKey { table, specification })?;
        Ok(())
    }

    pub fn drop_table(&self, table: TableId) -> PersistentCatalogResult<()> {
        self.commit(Command::DropTable { table })?;
        Ok(())
    }

    pub fn drop_schema(&self, schema: SchemaId) -> PersistentCatalogResult<()> {
        self.commit(Command::DropSchema { schema })?;
        Ok(())
    }

    pub fn drop_database(&self, database: DatabaseId) -> PersistentCatalogResult<()> {
        self.commit(Command::DropDatabase { database })?;
        Ok(())
    }
}

fn event_key(lsn: u64) -> String {
    // Fixed-width hexadecimal preserves the commit order within the RowStore
    // ordered binary namespace.
    format!("{EVENT_PREFIX}{lsn:016x}")
}
