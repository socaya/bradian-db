//! G23 explicit offline G16 shared-WAL -> G22 durable typed-history migration.
//!
//! Source G11 catalog and G15 shared-domain RowStore remain exclusively locked.
//! Migrate latest acknowledged data into an ABSENT sibling destination, through
//! an unpublished staging directory. No old historical row versions exist in
//! G16; the first G23 visible version is the import commit LSN per batch.
//! The source remains untouched in logical content; catalog G10 backup can
//! publish a source checkpoint. Live/online migration is unsupported.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::{Table, TableId};
use crate::durable_mvcc::{DurableMvccError, DurableMvccStore};
use crate::durable_typed::DurableTypedStore;
use crate::persistent_catalog::{PersistentCatalog, PersistentCatalogError};
use crate::row_store::{RowStore, StoreError};
use crate::transaction_domain::{DomainError, MultiTableStore};
use crate::typed_table::{encode_key, encode_row, validate_table, TableError, Value};

const SCHEMA_PREFIX: &[u8] = b"\0bradian-domain-table-v1:";
const BATCH_ROWS: usize = 96;

#[derive(Debug, Error)]
pub enum HistoryMigrationError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Catalog(#[from] PersistentCatalogError),
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error(transparent)]
    Durable(#[from] DurableMvccError),
    #[error(transparent)]
    Typed(#[from] TableError),
    #[error(transparent)]
    Storage(#[from] StoreError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("G23 migration refused: {0}")]
    Invalid(String),
    #[error("G23 migration verification failed: {0}")]
    Verification(String),
}

pub type HistoryMigrationResult<T> = Result<T, HistoryMigrationError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryMigrationReport {
    pub tables: usize,
    pub rows: usize,
    pub source_catalog_lsn: u64,
    pub source_domain_lsn: u64,
    pub destination_history_lsn: u64,
}

fn schema_key(id: TableId) -> Vec<u8> {
    let mut out = SCHEMA_PREFIX.to_vec();
    out.extend_from_slice(&id.0.to_be_bytes());
    out
}

fn physical_key(table: &Table, row: &[Value]) -> HistoryMigrationResult<Vec<u8>> {
    let positions = validate_table(table)?;
    let pk = positions.iter().map(|pos| row[*pos].clone()).collect::<Vec<_>>();
    Ok(encode_key(table, &positions, &pk)?)
}

fn fresh_root(
    source: &Path, requested: &Path,
) -> HistoryMigrationResult<(PathBuf, PathBuf)> {
    let src = fs::canonicalize(source)?;
    let parent = fs::canonicalize(requested.parent()
        .filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new(".")))?;
    let name = requested.file_name()
        .ok_or_else(|| HistoryMigrationError::Invalid("missing destination name".into()))?;
    let destination = parent.join(name);
    let staged = parent.join(format!(".{}.bradian-g23-incomplete", name.to_string_lossy()));
    for candidate in [&destination, &staged] {
        if candidate.starts_with(&src) || src.starts_with(candidate) {
            return Err(HistoryMigrationError::Invalid("destination overlaps source".into()));
        }
        if fs::symlink_metadata(candidate).is_ok() {
            return Err(HistoryMigrationError::Invalid(format!(
                "destination or unfinished staging already exists: {}", candidate.display()
            )));
        }
    }
    Ok((destination, staged))
}

fn sync_directory(root: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    { File::open(root)?.sync_all() }
    #[cfg(not(unix))]
    { let _ = root; Ok(()) }
}

/// The G16 source MUST be quiescent. Retains exclusive source catalog and
/// domain locks while copying, verifying and publishing a new G23 database.
/// No intermediate staging directory is treated as a valid user database.
pub fn migrate_shared_to_history(
    source_root: impl AsRef<Path>,
    destination_root: impl AsRef<Path>,
) -> HistoryMigrationResult<HistoryMigrationReport> {
    let source = fs::canonicalize(source_root.as_ref())?;
    if !source.is_dir() || !source.join("catalog").is_dir()
        || !source.join("shared-domain").is_dir()
    {
        return Err(HistoryMigrationError::Invalid(
            "source needs a catalog and registered G15 shared-domain".into(),
        ));
    }
    if source.join("history-domain").exists() || source.join("lifecycle").exists() {
        return Err(HistoryMigrationError::Invalid(
            "source is already historical or uses G18 lifecycle; backup-aware migration required".into(),
        ));
    }
    let legacy = source.join("tables");
    if legacy.exists() && fs::read_dir(legacy)?.next().is_some() {
        return Err(HistoryMigrationError::Invalid(
            "legacy per-table stores require G17 cutover before G23".into()
        ));
    }
    let (destination, staged) = fresh_root(&source, destination_root.as_ref())?;
    let catalog = PersistentCatalog::open(source.join("catalog"))?;
    let domain = MultiTableStore::open(source.join("shared-domain"))?;
    let catalog_lsn = catalog.last_lsn();
    let domain_lsn = domain.last_lsn();
    let catalog_tables = catalog.snapshot().tables()
        .map(|t| (t.id, t.clone())).collect::<BTreeMap<_,_>>();
    let registered = domain.registered_tables();
    let registered_ids = registered.iter().map(|t| t.id).collect::<BTreeSet<_>>();
    if catalog_tables.is_empty() || registered_ids.len() != catalog_tables.len()
        || registered_ids != catalog_tables.keys().copied().collect::<BTreeSet<_>>()
    {
        return Err(HistoryMigrationError::Invalid(
            "every catalog table must have exactly one registered source pin".into(),
        ));
    }
    for table in &registered {
        if catalog_tables.get(&table.id) != Some(table) {
            return Err(HistoryMigrationError::Verification(
                "source schema pin disagrees with catalog".into(),
            ));
        }
    }
    let tx = domain.begin_transaction()?;
    let mut source_rows = BTreeMap::new();
    let mut total_rows = 0;
    for table in &registered {
        let rows = tx.scan(table.id)?;
        total_rows += rows.len();
        source_rows.insert(table.id, rows);
    }
    if domain.last_lsn() != domain_lsn {
        return Err(HistoryMigrationError::Verification(
            "source changed while capturing snapshot".into()
        ));
    }

    fs::create_dir(&staged)?;
    let catalog_archive = staged.join("catalog-archive");
    catalog.backup_catalog_to(&catalog_archive)?;
    RowStore::restore_backup(&catalog_archive, staged.join("catalog"))?;
    fs::remove_dir_all(&catalog_archive)?;

    let history_path = staged.join("history-domain");
    let history = DurableMvccStore::open(&history_path)?;
    // Register schemas as durable, immutable user-key versions before rows.
    for chunk in registered.chunks(BATCH_ROWS) {
        let mut registration = history.begin();
        for table in chunk {
            registration.insert(&schema_key(table.id), &serde_json::to_vec(table)?)?;
        }
        registration.commit()?;
    }
    for table in &registered {
        for chunk in source_rows[&table.id].chunks(BATCH_ROWS) {
            let mut batch = history.begin();
            for row in chunk {
                batch.insert(&physical_key(table, row)?, &encode_row(table, row)?)?;
            }
            batch.commit()?;
        }
    }
    let destination_lsn = history.checkpoint()?;
    drop(history);

    // Verify independent WAL/checkpoint recovery before publication.
    {
        let copied = PersistentCatalog::open(staged.join("catalog"))?;
        if copied.last_lsn() != catalog_lsn
            || copied.snapshot().tables().cloned().collect::<Vec<_>>()
                != catalog.snapshot().tables().cloned().collect::<Vec<_>>()
        {
            return Err(HistoryMigrationError::Verification(
                "destination catalog LSN or table definitions differ".into()
            ));
        }
        let verified = DurableTypedStore::open(&history_path, &catalog_tables)?;
        if verified.last_lsn() != destination_lsn
            || verified.active_tables().len() != registered.len()
        {
            return Err(HistoryMigrationError::Verification(
                "destination history LSN/table pins mismatch".into()
            ));
        }
        let mut read = verified.begin();
        for table in &registered {
            if read.scan(table.id)? != source_rows[&table.id] {
                return Err(HistoryMigrationError::Verification(format!(
                    "migrated typed rows differ for table {}", table.id.0
                )));
            }
        }
    }
    if catalog.last_lsn() != catalog_lsn || domain.last_lsn() != domain_lsn {
        return Err(HistoryMigrationError::Verification(
            "source changed before publishing migration".into()
        ));
    }
    // Maintain source locks until after publication and sync. A concurrent
    // noncooperating destination creator remains an OS rename limitation.
    sync_directory(&staged)?;
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(HistoryMigrationError::Invalid(
            "destination appeared during cutover".into()
        ));
    }
    fs::rename(&staged, &destination)?;
    sync_directory(destination.parent().expect("canonical path has parent"))?;
    Ok(HistoryMigrationReport {
        tables: registered.len(),
        rows: total_rows,
        source_catalog_lsn: catalog_lsn,
        source_domain_lsn: domain_lsn,
        destination_history_lsn: destination_lsn,
    })
}
