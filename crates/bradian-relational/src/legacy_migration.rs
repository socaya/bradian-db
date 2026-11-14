//! G17: verified, offline legacy SQL -> shared-WAL database migration.
//!
//! The legacy row values and catalog definitions are not changed or renamed. Its catalog and EVERY
//! existing typed-table RowStore are exclusively locked for the entire export.
//! A new destination is populated in a sibling staging directory, then
//! reopened and verified before an atomic directory rename publishes it.
//!
//! This is a COPY-and-VERIFY offline cutover, not live dual-write migration,
//! in-place mutation, or rollback of writes made after an application switches
//! to the new destination. The owner must stop all legacy SQL clients first.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::{Table, TableId};
use crate::persistent_catalog::{PersistentCatalog, PersistentCatalogError};
use crate::row_store::{RowStore, StoreError};
use crate::runtime_limits::DirectoryLimits;
use crate::transaction_domain::{DomainError, MultiTableStore};
use crate::typed_table::{TableError, TableStore, Value};

const IMPORT_BATCH: usize = 128;

#[derive(Debug, Error)]
pub enum MigrationError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Catalog(#[from] PersistentCatalogError),
    #[error(transparent)]
    Table(#[from] TableError),
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error(transparent)]
    Storage(#[from] StoreError),
    #[error("legacy migration refused: {0}")]
    Invalid(String),
    #[error("migration verification failed: {0}")]
    Verification(String),
}

pub type MigrationResult<T> = Result<T, MigrationError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableMigration {
    pub table_id: TableId,
    pub rows: usize,
    /// Zero indicates that this table had no provisioned legacy RowStore.
    pub legacy_lsn: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    pub tables: Vec<TableMigration>,
    pub total_rows: usize,
    pub catalog_lsn: u64,
    pub shared_domain_lsn: u64,
}

fn directory_sync(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    { File::open(path)?.sync_all() }
    #[cfg(not(unix))]
    { let _ = path; Ok(()) }
}

/// A staged table owns its original legacy file lock until the destination
/// has been fully verified and published.
struct SourceTable {
    definition: Table,
    source: Option<TableStore>,
    rows: Vec<Vec<Value>>,
    legacy_lsn: u64,
}

fn validate_destination(source: &Path, destination: &Path) -> MigrationResult<(PathBuf, PathBuf)> {
    let src = fs::canonicalize(source)?;
    if !src.is_dir() {
        return Err(MigrationError::Invalid("source is not a directory".into()));
    }
    let parent = destination.parent().filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent)?;
    let filename = destination.file_name()
        .ok_or_else(|| MigrationError::Invalid("destination must have a filename".into()))?;
    let actual_destination = parent.join(filename);
    if actual_destination.starts_with(&src) || src.starts_with(&actual_destination)
        || fs::symlink_metadata(&actual_destination).is_ok()
    {
        return Err(MigrationError::Invalid("destination overlaps source or already exists".into()));
    }
    let staging = parent.join(format!(".{}.bradian-g17-incomplete", filename.to_string_lossy()));
    if fs::symlink_metadata(&staging).is_ok() {
        return Err(MigrationError::Invalid("previous migration staging directory exists".into()));
    }
    if staging.starts_with(&src) || src.starts_with(&staging) {
        return Err(MigrationError::Invalid("staging overlaps source".into()));
    }
    Ok((actual_destination, staging))
}

fn source_table_ids(tables_dir: &Path, directory_limits: DirectoryLimits)
    -> MigrationResult<BTreeSet<TableId>>
{
    if !tables_dir.exists() { return Ok(BTreeSet::new()); }
    let mut ids = BTreeSet::new();
    let mut entries = 0_usize;
    let mut name_bytes = 0_usize;
    for entry in fs::read_dir(tables_dir)? {
        let entry = entry?;
        let file = entry.file_name();
        let encoded_len = file.as_encoded_bytes().len();
        (entries, name_bytes) = directory_limits.admit(entries, name_bytes, encoded_len)
            .map_err(StoreError::from)?;
        if !entry.file_type()?.is_dir() {
            return Err(MigrationError::Invalid("unexpected file in legacy tables directory".into()));
        }
        let name = file.to_str()
            .ok_or_else(|| MigrationError::Invalid("non-UTF8 table directory".into()))?;
        let id = name.parse::<u64>()
            .map_err(|_| MigrationError::Invalid(format!("unexpected legacy directory {name}")))?;
        if name != id.to_string() || !ids.insert(TableId(id)) {
            return Err(MigrationError::Invalid("noncanonical or duplicate table directory".into()));
        }
    }
    Ok(ids)
}

fn verify_rows(
    domain: &MultiTableStore, sources: &[SourceTable],
) -> MigrationResult<()> {
    let tx = domain.begin_transaction()?;
    for source in sources {
        let actual = tx.scan(source.definition.id)?;
        if actual != source.rows {
            return Err(MigrationError::Verification(format!(
                "row content/order differs for table {}", source.definition.id.0
            )));
        }
    }
    Ok(())
}

/// Copy a stopped legacy SQL database to a separate, verified shared-domain
/// directory. Source logical rows and definitions are preserved, but catalog backup may
/// publish a source checkpoint. Source catalog and all existing
/// table file locks remain held until the destination is published.
///
/// On failure the destination does not appear; an incomplete staging directory
/// is intentionally retained for forensic inspection, not automatically reused.
pub fn migrate_legacy_sql(
    source_root: impl AsRef<Path>,
    destination_root: impl AsRef<Path>,
) -> MigrationResult<MigrationReport> {
    migrate_legacy_sql_with_policies(source_root, destination_root, DirectoryLimits::default())
}

pub fn migrate_legacy_sql_with_policies(
    source_root: impl AsRef<Path>,
    destination_root: impl AsRef<Path>,
    directory_limits: DirectoryLimits,
) -> MigrationResult<MigrationReport> {
    let source_root = fs::canonicalize(source_root.as_ref())?;
    let (destination, staging) = validate_destination(&source_root, destination_root.as_ref())?;
    // A G15/G16 source already has authoritative shared-WAL rows. Treating
    // it as a G14 legacy root could silently omit its committed data.
    if fs::symlink_metadata(source_root.join("shared-domain")).is_ok() {
        return Err(MigrationError::Invalid(
            "source already has shared-domain storage; legacy migration is not applicable".into(),
        ));
    }
    let catalog_dir = source_root.join("catalog");
    if !catalog_dir.is_dir() {
        return Err(MigrationError::Invalid("legacy catalog directory is missing".into()));
    }
    let catalog = PersistentCatalog::open(&catalog_dir)?;
    // The catalog backup below uses G10 and may checkpoint the original
    // catalog. This does not change DDL events or acknowledged logical rows.
    let catalog_snapshot = catalog.snapshot();
    let catalog_lsn = catalog.last_lsn();
    let definitions = catalog_snapshot.tables().cloned().collect::<Vec<_>>();
    let known = definitions.iter().map(|table| table.id).collect::<BTreeSet<_>>();
    let existing = source_table_ids(&source_root.join("tables"), directory_limits)?;
    for id in &existing {
        if !known.contains(id) {
            return Err(MigrationError::Invalid(format!(
                "orphan legacy table directory {}", id.0
            )));
        }
    }

    // Acquire ALL existing legacy table locks before copying any row values.
    // An external legacy writer with one lock held causes migration to fail,
    // rather than capturing records from different points in time.
    let mut sources = Vec::with_capacity(definitions.len());
    for definition in definitions {
        let id = definition.id;
        let source = if existing.contains(&id) {
            Some(TableStore::open(
                source_root.join("tables").join(id.0.to_string()),
                definition.clone(),
            )?)
        } else { None };
        let (rows, legacy_lsn) = match &source {
            Some(table) => (table.scan()?, table.last_lsn()),
            None => (Vec::new(), 0),
        };
        sources.push(SourceTable { definition, source, rows, legacy_lsn });
    }

    // Recheck each locked source row image before publication. Retain the
    // handles and catalog lock until after final rename.
    fs::create_dir(&staging)?;
    let archive = staging.join("catalog-archive");
    catalog.backup_catalog_to(&archive)?;
    RowStore::restore_backup(&archive, staging.join("catalog"))?;
    fs::remove_dir_all(&archive)?;

    let domain_path = staging.join("shared-domain");
    let domain = MultiTableStore::open(&domain_path)?;
    let mut total_rows = 0_usize;
    for source in &sources {
        domain.register_table(source.definition.clone())?;
        total_rows += source.rows.len();
        for batch in source.rows.chunks(IMPORT_BATCH) {
            let mut transaction = domain.begin_transaction()?;
            for row in batch {
                transaction.insert(source.definition.id, row)?;
            }
            transaction.commit()?;
        }
    }
    verify_rows(&domain, &sources)?;
    let shared_domain_lsn = domain.checkpoint()?;
    drop(domain);

    // Reopening must replay the durable catalog and domain successfully;
    // compare all rows and object identities with the still-locked originals.
    {
        let reopened_catalog = PersistentCatalog::open(staging.join("catalog"))?;
        if reopened_catalog.last_lsn() != catalog_lsn {
            return Err(MigrationError::Verification("catalog LSN changed".into()));
        }
        let actual = reopened_catalog.snapshot();
        let actual_tables = actual.tables().cloned().collect::<Vec<_>>();
        let expected_tables = sources.iter().map(|s| s.definition.clone()).collect::<Vec<_>>();
        if actual_tables != expected_tables {
            return Err(MigrationError::Verification("catalog definitions differ".into()));
        }
    }
    {
        let reopened_domain = MultiTableStore::open(&domain_path)?;
        if reopened_domain.last_lsn() != shared_domain_lsn ||
            reopened_domain.table_count() != sources.len()
        {
            return Err(MigrationError::Verification("domain checkpoint or table count changed".into()));
        }
        verify_rows(&reopened_domain, &sources)?;
    }

    for source in &sources {
        if let Some(legacy) = &source.source {
            if legacy.last_lsn() != source.legacy_lsn || legacy.scan()? != source.rows {
                return Err(MigrationError::Verification(format!(
                    "source changed during migration: table {}", source.definition.id.0
                )));
            }
        }
    }
    if catalog.last_lsn() != catalog_lsn {
        return Err(MigrationError::Verification("source catalog changed during migration".into()));
    }

    directory_sync(&staging)?;
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(MigrationError::Invalid(
            "destination appeared during migration; refusing replacement".into(),
        ));
    }
    // Directory rename is atomic on supported local filesystems, but a
    // portable no-replace rename is not supplied by std::fs. Deployment
    // qualification must prevent competing destination creators.
    fs::rename(&staging, &destination)?;
    directory_sync(destination.parent().expect("canonical destination has parent"))?;
    Ok(MigrationReport {
        tables: sources.iter().map(|s| TableMigration {
            table_id: s.definition.id,
            rows: s.rows.len(),
            legacy_lsn: s.legacy_lsn,
        }).collect(),
        total_rows,
        catalog_lsn,
        shared_domain_lsn,
    })
}
