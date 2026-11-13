//! G18: durable table provisioning lifecycle between the G11 catalog and
//! G15 shared-WAL domain. Not a distributed atomic DDL transaction.
//!
//! A PENDING intent is WAL-synced before a domain schema pin is written.
//! The domain pin is WAL-synced before the lifecycle journal changes to
//! ACTIVE. Restart idempotently reconciles incomplete intents against the
//! existing durable catalog. Mismatched or orphaned pins fail closed.
//!
//! This coordinator must be the only writer of schema lifecycle metadata
//! for its root. It does not permit concurrent independent SqlEngine catalog
//! DDL, does not alter/drop schemas, and does not migrate legacy table files.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use parking_lot::Mutex;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Table, TableId};
use crate::persistent_catalog::{PersistentCatalog, PersistentCatalogError};
use crate::row_store::{RowStore, StoreError};
use crate::transaction_domain::{DomainError, MultiTableStore};

const MARKER: &[u8] = b"bradian.lifecycle.format";
const VERSION: &[u8] = b"BRADIAN-TABLE-LIFECYCLE-V1";
const PREFIX: &str = "table/";
const MAX_RETRIES: usize = 16;

#[derive(Debug, Error)]
pub enum LifecycleError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Catalog(#[from] PersistentCatalogError),
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error(transparent)]
    Storage(#[from] StoreError),
    #[error(transparent)]
    Encoding(#[from] serde_json::Error),
    #[error("table lifecycle integrity violation: {0}")]
    Integrity(String),
    #[error("table id {0} is not defined in the durable catalog")]
    MissingCatalogTable(u64),
    #[error("table id {0} is not ready for domain access")]
    NotActive(u64),
}

pub type LifecycleResult<T> = Result<T, LifecycleError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TableLifecycleState {
    Pending,
    Active,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    version: u8,
    table: Table,
    state: TableLifecycleState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableLifecycleStatus {
    pub table_id: TableId,
    pub state: TableLifecycleState,
}

fn record_key(id: TableId) -> Vec<u8> {
    format!("{PREFIX}{:016x}", id.0).into_bytes()
}

fn catalog_tables(catalog: &PersistentCatalog) -> BTreeMap<TableId, Table> {
    catalog.snapshot().tables()
        .map(|table| (table.id, table.clone()))
        .collect()
}

/// Single-process lifecycle owner. Opening it performs recovery before
/// returning a usable object. The exclusive locks on catalog, domain, and
/// lifecycle journal are held until this owner is dropped.
pub struct CatalogLifecycle {
    root: PathBuf,
    catalog: PersistentCatalog,
    domain: MultiTableStore,
    journal: RowStore,
    operation: Mutex<()>,
}

impl CatalogLifecycle {
    pub fn open(root: impl AsRef<Path>) -> LifecycleResult<Self> {
        if root.as_ref().join("history-domain").exists() {
            return Err(LifecycleError::Integrity(
                "G23 durable history root requires HistorySqlEngine; legacy SQL ownership refused".into()
            ));
        }
        let root = root.as_ref();
        let legacy = root.join("tables");
        if legacy.exists() && fs::read_dir(&legacy)?.next().is_some() {
            return Err(LifecycleError::Integrity(
                "legacy per-table storage present; complete G17 migration first".into(),
            ));
        }
        fs::create_dir_all(root)?;
        let catalog = PersistentCatalog::open(root.join("catalog"))?;
        let domain = MultiTableStore::open(root.join("shared-domain"))?;
        let journal = RowStore::open(root.join("lifecycle"))?;
        if journal.is_empty() {
            journal.put(MARKER, VERSION)?;
        } else if journal.get(MARKER).as_deref() != Some(VERSION) {
            return Err(LifecycleError::Integrity(
                "missing or unsupported lifecycle format marker".into(),
            ));
        }
        let owner = Self {
            root: root.to_path_buf(),
            catalog, domain, journal,
            operation: Mutex::new(()),
        };
        owner.reconcile()?;
        Ok(owner)
    }

    pub fn root(&self) -> &Path { &self.root }
    pub fn last_domain_lsn(&self) -> u64 { self.domain.last_lsn() }
    pub fn last_lifecycle_lsn(&self) -> u64 { self.journal.last_lsn() }

    pub(crate) fn catalog_backup_identity(&self) -> (u64, u64) {
        (self.catalog.last_lsn(), self.catalog.journal_row_count())
    }

    pub(crate) fn domain_backup_identity(&self) -> (u64, u64) {
        (self.domain.last_lsn(), self.domain.storage_row_count())
    }

    pub(crate) fn lifecycle_backup_identity(&self) -> (u64, u64) {
        (self.journal.last_lsn(), self.journal.len() as u64)
    }

    fn load_records(&self) -> LifecycleResult<BTreeMap<TableId, Record>> {
        let mut records = BTreeMap::new();
        for (key, data) in self.journal.scan() {
            if key.as_slice() == MARKER { continue; }
            let text = std::str::from_utf8(&key)
                .map_err(|_| LifecycleError::Integrity("non-UTF8 lifecycle key".into()))?;
            let Some(id_str) = text.strip_prefix(PREFIX) else {
                return Err(LifecycleError::Integrity("unexpected lifecycle key namespace".into()));
            };
            if id_str.len() != 16 || !id_str.bytes().all(|c| c.is_ascii_hexdigit()) ||
                id_str.bytes().any(|c| c.is_ascii_uppercase())
            {
                return Err(LifecycleError::Integrity("invalid lifecycle key format".into()));
            }
            let id = TableId(u64::from_str_radix(id_str, 16)
                .map_err(|_| LifecycleError::Integrity("invalid lifecycle table identity".into()))?);
            let entry: Record = serde_json::from_slice(&data)
                .map_err(|e| LifecycleError::Integrity(format!("invalid lifecycle record: {e}")))?;
            if entry.version != 1 || entry.table.id != id {
                return Err(LifecycleError::Integrity("lifecycle identity/version mismatch".into()));
            }
            if records.insert(id, entry).is_some() {
                return Err(LifecycleError::Integrity("duplicate lifecycle identity".into()));
            }
        }
        Ok(records)
    }

    fn set_state(&self, table: Table, state: TableLifecycleState) -> LifecycleResult<u64> {
        let key = record_key(table.id);
        let expected = Record { version: 1, table, state };
        let encoded = serde_json::to_vec(&expected)?;
        for _ in 0..MAX_RETRIES {
            let lsn = self.journal.last_lsn();
            match self.journal.commit_batch(Some(lsn), &[
                crate::row_store::RowMutation::Put {
                    key: key.clone(), value: encoded.clone()
                },
            ]) {
                Ok(lsn) => return Ok(lsn),
                Err(StoreError::Conflict { .. }) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(LifecycleError::Integrity(
            "concurrent lifecycle writer prevented state publication".into(),
        ))
    }

    /// Reconstruct unfinished registrations and verify that active schema
    /// pins match the durable catalog. An orphan pin or stale schema is a
    /// hard failure, never silently pruned or rewritten.
    pub fn reconcile(&self) -> LifecycleResult<Vec<TableLifecycleStatus>> {
        let _guard = self.operation.lock();
        self.reconcile_locked()
    }

    pub(crate) fn reconcile_locked(&self) -> LifecycleResult<Vec<TableLifecycleStatus>> {
        let catalog = catalog_tables(&self.catalog);
        let mut records = self.load_records()?;
        let pins = self.domain.registered_tables();
        let mut pin_ids = BTreeSet::new();
        for pin in &pins {
            let expected = catalog.get(&pin.id)
                .ok_or_else(|| LifecycleError::Integrity(
                    format!("orphan domain pin for missing catalog table {}", pin.id.0)
                ))?;
            if expected != pin {
                return Err(LifecycleError::Integrity(
                    format!("pinned schema disagrees with catalog table {}", pin.id.0)
                ));
            }
            pin_ids.insert(pin.id);
        }
        for (&id, record) in &records {
            let expected = catalog.get(&id)
                .ok_or(LifecycleError::MissingCatalogTable(id.0))?;
            if expected != &record.table {
                return Err(LifecycleError::Integrity(
                    format!("lifecycle schema differs from catalog table {}", id.0)
                ));
            }
        }
        // Older G16 databases have registered pins but no G18 journal.
        // Adopt only matching schemas; this records an ACTIVE state but
        // never changes authoritative user row data.
        for pin in pins {
            if let std::collections::btree_map::Entry::Vacant(e) = records.entry(pin.id) {
                self.set_state(pin.clone(), TableLifecycleState::Active)?;
                e.insert(Record {
                    version: 1, table: pin,
                    state: TableLifecycleState::Active,
                });
            }
        }
        // Complete an interrupted PENDING intent. Domain registration is
        // idempotent, so the crash may have occurred either before or after
        // its durable schema pin was written.
        for record in records.values_mut() {
            if record.state == TableLifecycleState::Pending {
                self.domain.register_table(record.table.clone())?;
                self.set_state(record.table.clone(), TableLifecycleState::Active)?;
                record.state = TableLifecycleState::Active;
            } else if !pin_ids.contains(&record.table.id) {
                return Err(LifecycleError::Integrity(format!(
                    "ACTIVE table {} has no domain schema pin", record.table.id.0
                )));
            }
        }
        Ok(records.into_iter().map(|(table_id, record)| TableLifecycleStatus {
            table_id, state: record.state,
        }).collect())
    }

    /// Register a table already present in the durable catalog.
    /// Steps: (1) journal PENDING; (2) domain schema pin; (3) journal ACTIVE.
    /// A crash between steps is resolved deterministically on restart.
    /// No table DATA is modified by provisioning.
    pub fn provision_existing(&self, id: TableId) -> LifecycleResult<u64> {
        let _guard = self.operation.lock();
        let catalog = catalog_tables(&self.catalog);
        let table = catalog.get(&id)
            .ok_or(LifecycleError::MissingCatalogTable(id.0))?.clone();
        if let Some(record) = self.load_records()?.get(&id) {
            if record.table != table {
                return Err(LifecycleError::Integrity("existing lifecycle schema mismatch".into()));
            }
            if record.state == TableLifecycleState::Active {
                let pin = self.domain.registered_tables().into_iter()
                    .find(|t| t.id == id);
                if pin.as_ref() == Some(&table) {
                    return Ok(self.domain.last_lsn());
                }
                return Err(LifecycleError::Integrity("ACTIVE table lost its physical pin".into()));
            }
        } else {
            self.set_state(table.clone(), TableLifecycleState::Pending)?;
        }
        let lsn = self.domain.register_table(table.clone())?;
        self.set_state(table, TableLifecycleState::Active)?;
        Ok(lsn)
    }

    /// Capture one reconciled catalog/domain/lifecycle database image while
    /// holding the coordinator operation mutex and all three RowStore locks.
    /// This is consistent for cooperating local clients only: it is not a
    /// distributed snapshot or an online lock-free backup.
    pub fn backup_database_to(
        &self,
        destination: impl AsRef<Path>,
    ) -> Result<crate::database_backup::DatabaseBackupReport, crate::database_backup::DatabaseBackupError> {
        self.backup_database_to_with_metadata_limits(destination,crate::runtime_limits::MetadataLimits::default())
    }

    pub fn backup_database_to_with_metadata_limits(
        &self, destination: impl AsRef<Path>, metadata: crate::runtime_limits::MetadataLimits,
    ) -> Result<crate::database_backup::DatabaseBackupReport,crate::database_backup::DatabaseBackupError> {
        let _guard = self.operation.lock();
        let statuses = self.reconcile_locked()?;
        crate::database_backup::backup_database(
            &self.root, destination.as_ref(), &self.catalog,
            &self.domain, &self.journal, &statuses, metadata,
        )
    }

    pub fn state(&self, id: TableId) -> LifecycleResult<Option<TableLifecycleState>> {
        Ok(self.load_records()?.get(&id).map(|r| r.state))
    }
}
