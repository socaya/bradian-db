//! G19 coordinated, independently verified local database backups.
//!
//! One G18 CatalogLifecycle owns the G11 catalog, G15 shared domain and G18
//! lifecycle file locks for the entire backup. A fourth, versioned manifest
//! ties together the three independently recoverable G10 backup receipts.
//! Restore copies each verified store into a staging root and opens the
//! complete G18 coordinator to verify schema pins and lifecycle state before
//! publishing the destination. Existing databases are never overwritten.
//!
//! No claim is made of a remote/distributed backup, MVCC snapshot, PITR,
//! encrypted archives, or protection against noncooperating filesystem writers.

use crate::runtime_limits::MetadataLimits;
use crate::metadata_io;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crc32fast::Hasher;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::backup::{self, BackupReceipt};
use crate::lifecycle::{
    CatalogLifecycle, LifecycleError, TableLifecycleState, TableLifecycleStatus,
};
use crate::persistent_catalog::{PersistentCatalog, PersistentCatalogError};
use crate::row_store::{BaceWiring, RowStore, StoreError};
use crate::runtime_limits::StorageLimits;
use crate::transaction_domain::{DomainError, MultiTableStore};

const FORMAT: &str = "BRADIAN-DATABASE-BACKUP-V1";
const MANIFEST: &str = "database-backup.json";
const COMPONENTS: [&str; 3] = ["catalog", "shared-domain", "lifecycle"];

#[derive(Debug, Error)]
pub enum DatabaseBackupError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Catalog(#[from] PersistentCatalogError),
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    #[error(transparent)]
    Storage(#[from] StoreError),
    #[error(transparent)]
    Encoding(#[from] serde_json::Error),
    #[error("database backup refused: {0}")]
    Invalid(String),
    #[error("database backup integrity violation: {0}")]
    Corruption(String),
    #[error("database backup carries BACE v2 pages in one or more components; restore_database_with_bace is required")]
    RestoreRequiresBaceWiring,
}

pub type DatabaseBackupResult<T> = Result<T, DatabaseBackupError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentReceipt {
    pub checkpoint_lsn: u64,
    pub last_lsn: u64,
    pub rows: u64,
    /// CRC32 over exactly the five G10 archive files (names and bytes).
    /// Detects accidental substitution/damage, not malicious forgery.
    pub archive_crc32: u32,
}

impl From<BackupReceipt> for ComponentReceipt {
    fn from(receipt: BackupReceipt) -> Self {
        Self {
            checkpoint_lsn: receipt.checkpoint_lsn,
            last_lsn: receipt.last_lsn,
            rows: receipt.rows,
            archive_crc32: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseBackupReport {
    pub catalog: ComponentReceipt,
    pub domain: ComponentReceipt,
    pub lifecycle: ComponentReceipt,
    pub active_tables: Vec<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestPayload {
    format: String,
    report: DatabaseBackupReport,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    payload: ManifestPayload,
    crc32: u32,
}

fn sync_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    { File::open(path)?.sync_all() }
    #[cfg(not(unix))]
    { let _ = path; Ok(()) }
}

fn canonical_new_destination(
    destination: &Path, forbidden: &Path,
) -> DatabaseBackupResult<(PathBuf, PathBuf)> {
    let parent = destination.parent().filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent)?;
    let name = destination.file_name()
        .ok_or_else(|| DatabaseBackupError::Invalid("destination must have a filename".into()))?;
    let dest = parent.join(name);
    let excluded = fs::canonicalize(forbidden)?;
    if dest.starts_with(&excluded) || excluded.starts_with(&dest) {
        return Err(DatabaseBackupError::Invalid("source/destination overlap".into()));
    }
    if fs::symlink_metadata(&dest).is_ok() {
        return Err(DatabaseBackupError::Invalid("destination exists".into()));
    }
    let staged = parent.join(format!(".{}.bradian-g19-incomplete", name.to_string_lossy()));
    if fs::symlink_metadata(&staged).is_ok() {
        return Err(DatabaseBackupError::Invalid(
            "previous incomplete database backup staging directory exists".into(),
        ));
    }
    if staged.starts_with(&excluded) || excluded.starts_with(&staged) {
        return Err(DatabaseBackupError::Invalid("staging overlaps source".into()));
    }
    Ok((dest, staged))
}

#[derive(Serialize)]
struct PayloadRef<'a> { format: &'static str, report: &'a DatabaseBackupReport }
#[derive(Serialize)]
struct EnvelopeRef<'a> { payload: PayloadRef<'a>, crc32: u32 }
fn write_manifest(root: &Path, report: &DatabaseBackupReport, limits: MetadataLimits) -> Result<(),DatabaseBackupError> {
    let payload=PayloadRef { format:FORMAT,report };
    let crc32=metadata_io::json_crc(&payload,limits)?;
    let bytes=metadata_io::json_bytes(&EnvelopeRef { payload,crc32 },limits)?;
    let mut file=OpenOptions::new().write(true).create_new(true).open(root.join(MANIFEST))?;
    file.write_all(&bytes)?; file.sync_all()?; sync_dir(root)?; Ok(())
}

fn read_manifest(root: &Path, limits: MetadataLimits) -> DatabaseBackupResult<DatabaseBackupReport> {
    let bytes=metadata_io::read_json_bytes(&root.join(MANIFEST),limits).map_err(|error|match error {
        StoreError::Io(error)=>DatabaseBackupError::Io(error), other=>DatabaseBackupError::Storage(other),
    })?;
    let envelope: Envelope = serde_json::from_slice(&bytes)
        .map_err(|e| DatabaseBackupError::Corruption(format!("manifest JSON: {e}")))?;
    if envelope.payload.format != FORMAT {
        return Err(DatabaseBackupError::Corruption("unsupported database backup version".into()));
    }
    if metadata_io::json_crc(&envelope.payload,limits)? != envelope.crc32 {
        return Err(DatabaseBackupError::Corruption("database backup manifest CRC mismatch".into()));
    }
    if envelope.payload.report.active_tables.windows(2).any(|w| w[0] >= w[1]) {
        return Err(DatabaseBackupError::Corruption(
            "table identity list is not strictly ordered".into(),
        ));
    }
    for receipt in [
        envelope.payload.report.catalog,
        envelope.payload.report.domain,
        envelope.payload.report.lifecycle,
    ] {
        if receipt.last_lsn < receipt.checkpoint_lsn {
            return Err(DatabaseBackupError::Corruption("backup LSN regression".into()));
        }
    }
    Ok(envelope.payload.report)
}

/// Bind each component receipt to its exact backed-up bytes. Streaming
/// verification avoids allocating whole snapshots and WALs in memory.
fn archive_crc(root: &Path, receipt: ComponentReceipt) -> DatabaseBackupResult<u32> {
    let files = [
        "backup.info".to_owned(),
        "checkpoint.current".to_owned(),
        format!("snapshot-{}.brp", receipt.checkpoint_lsn),
        format!("index-{}.bri", receipt.checkpoint_lsn),
        "rows.wal".to_owned(),
    ];
    let mut checksum = Hasher::new();
    for filename in files {
        checksum.update(&(filename.len() as u32).to_le_bytes());
        checksum.update(filename.as_bytes());
        let mut file = File::open(root.join(&filename))?;
        let mut chunk = [0_u8; 65536];
        loop {
            let n = file.read(&mut chunk)?;
            if n == 0 { break; }
            checksum.update(&chunk[..n]);
        }
    }
    Ok(checksum.finalize())
}

fn verify_archive_crc(
    root: &Path,
    receipt: ComponentReceipt,
) -> DatabaseBackupResult<()> {
    if archive_crc(root, receipt)? != receipt.archive_crc32 {
        return Err(DatabaseBackupError::Corruption(format!(
            "component archive checksum differs: {}", root.display()
        )));
    }
    Ok(())
}

fn receipt_matches(actual: BackupReceipt, expected: ComponentReceipt) -> bool {
    actual.checkpoint_lsn == expected.checkpoint_lsn
        && actual.last_lsn == expected.last_lsn && actual.rows == expected.rows
}

fn verify_reopened(
    root: &Path, report: &DatabaseBackupReport,
    bace: Option<&BaceWiring>,
) -> DatabaseBackupResult<()> {
    let owner = match bace {
        Some(w) => CatalogLifecycle::open_with_bace(root, w.clone())?,
        None => CatalogLifecycle::open(root)?,
    };
    let statuses = owner.reconcile()?;
    if statuses.iter().any(|status| status.state != TableLifecycleState::Active) {
        return Err(DatabaseBackupError::Corruption("restored table is not ACTIVE".into()));
    }
    let ids = statuses.iter().map(|status| status.table_id.0).collect::<Vec<_>>();
    if ids != report.active_tables {
        return Err(DatabaseBackupError::Corruption("restored active table identities differ".into()));
    }
    if owner.catalog_backup_identity() != (report.catalog.last_lsn, report.catalog.rows)
        || owner.domain_backup_identity() != (report.domain.last_lsn, report.domain.rows)
        || owner.lifecycle_backup_identity() != (report.lifecycle.last_lsn, report.lifecycle.rows)
    {
        return Err(DatabaseBackupError::Corruption(
            "restored catalog/domain/lifecycle row counts or LSNs differ".into(),
        ));
    }
    Ok(())
}

/// Called by CatalogLifecycle while holding its operation mutex and the three
/// native RowStore exclusive locks; component backup operations are therefore
/// one consistent local logical cut across cooperating clients.
// The coordinator-backup signature is intrinsic to the G19 contract:
// four distinct sub-components, one lifecycle status slice, metadata
// limits, and the ADR 0069 BACE wiring. Grouping these into a struct
// would obscure the call site more than clarify it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn backup_database(
    source_root: &Path,
    destination: &Path,
    catalog: &PersistentCatalog,
    domain: &MultiTableStore,
    lifecycle: &RowStore,
    statuses: &[TableLifecycleStatus],
    metadata: MetadataLimits,
    bace: Option<&BaceWiring>,
) -> DatabaseBackupResult<DatabaseBackupReport> {
    if statuses.iter().any(|status| status.state != TableLifecycleState::Active) {
        return Err(DatabaseBackupError::Invalid("unreconciled lifecycle state".into()));
    }
    let (destination, staged) = canonical_new_destination(destination, source_root)?;
    fs::create_dir(&staged)?;
    // Each G10 component backup is fully validated before the next one.
    // The source coordinator exclusively owns all component WAL locks.
    let catalog_receipt = catalog.backup_catalog_to(staged.join(COMPONENTS[0]))?;
    let domain_receipt = domain.backup_to(staged.join(COMPONENTS[1]))?;
    let lifecycle_receipt = lifecycle.backup_to(staged.join(COMPONENTS[2]))?;
    let mut catalog_point = ComponentReceipt::from(catalog_receipt);
    let mut domain_point = ComponentReceipt::from(domain_receipt);
    let mut lifecycle_point = ComponentReceipt::from(lifecycle_receipt);
    catalog_point.archive_crc32 = archive_crc(&staged.join(COMPONENTS[0]), catalog_point)?;
    domain_point.archive_crc32 = archive_crc(&staged.join(COMPONENTS[1]), domain_point)?;
    lifecycle_point.archive_crc32 = archive_crc(&staged.join(COMPONENTS[2]), lifecycle_point)?;
    let report = DatabaseBackupReport {
        catalog: catalog_point,
        domain: domain_point,
        lifecycle: lifecycle_point,
        active_tables: statuses.iter().map(|s| s.table_id.0).collect(),
    };
    write_manifest(&staged, &report, metadata)?;
    // Rebuild, replay and compare the actual relational catalog, physical
    // domain schema pins and lifecycle journal in the independently copied
    // backup before publishing it.
    // A G10 component backup is itself a directory of RowStore files, not a
    // live database root. Restore each one into an additional temporary root.
    //
    // ADR 0069 — when the coordinator was opened with BACE wiring, the
    // sub-archives carry BACE v2 pages (flag set on each backup.info).
    // We must restore via `restore_backup_with_bace`; the legacy
    // `restore_backup` would fire `BackupRestoreRequiresBaceWiring`.
    let verify = staged.join("verified-root");
    fs::create_dir(&verify)?;
    for component in COMPONENTS {
        match bace {
            Some(w) => {
                RowStore::restore_backup_with_bace(
                    staged.join(component), verify.join(component),
                    StorageLimits::default(), w.clone(),
                )?;
            }
            None => {
                RowStore::restore_backup(staged.join(component), verify.join(component))?;
            }
        }
    }
    verify_reopened(&verify, &report, bace)?;
    fs::remove_dir_all(&verify)?;
    sync_dir(&staged)?;
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(DatabaseBackupError::Invalid("destination appeared before publication".into()));
    }
    fs::rename(&staged, &destination)?;
    sync_dir(destination.parent().expect("canonical destination has parent"))?;
    Ok(report)
}

/// Restore a complete G19 backup into an absent database root. Each G10
/// archive is first restored independently into a staging root; the G18
/// coordinator then verifies cross-component table pins and lifecycle state.
/// A fully validated directory is published only after all checks succeed.
pub fn restore_database(
    backup: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> DatabaseBackupResult<DatabaseBackupReport> {
    restore_database_with_metadata_limits(backup,destination,MetadataLimits::default())
}

pub fn restore_database_with_metadata_limits(
    backup: impl AsRef<Path>, destination: impl AsRef<Path>, metadata: MetadataLimits,
) -> DatabaseBackupResult<DatabaseBackupReport> {
    restore_database_inner(backup.as_ref(), destination.as_ref(), metadata, None)
}

/// ADR 0068 §3.1 — BACE-aware coordinator restore. Required when any
/// sub-archive's `backup.info` declares `payload_has_bace_pages = true`.
/// Tolerated on legacy (all-flag-clear) coordinator backups: per
/// ADR 0065 §3.5, `open_with_bace` works on v1-only directories.
pub fn restore_database_with_bace(
    backup: impl AsRef<Path>,
    destination: impl AsRef<Path>,
    wiring: BaceWiring,
) -> DatabaseBackupResult<DatabaseBackupReport> {
    restore_database_with_bace_and_metadata_limits(
        backup, destination, MetadataLimits::default(), wiring,
    )
}

pub fn restore_database_with_bace_and_metadata_limits(
    backup: impl AsRef<Path>,
    destination: impl AsRef<Path>,
    metadata: MetadataLimits,
    wiring: BaceWiring,
) -> DatabaseBackupResult<DatabaseBackupReport> {
    restore_database_inner(backup.as_ref(), destination.as_ref(), metadata, Some(wiring))
}

fn restore_database_inner(
    backup: &Path,
    destination: &Path,
    metadata: MetadataLimits,
    wiring: Option<BaceWiring>,
) -> DatabaseBackupResult<DatabaseBackupReport> {
    let backup = fs::canonicalize(backup)?;
    if !backup.is_dir() {
        return Err(DatabaseBackupError::Invalid("backup is not a directory".into()));
    }
    let report = read_manifest(&backup,metadata)?;
    for (component, expected) in [
        (COMPONENTS[0], report.catalog),
        (COMPONENTS[1], report.domain),
        (COMPONENTS[2], report.lifecycle),
    ] {
        verify_archive_crc(&backup.join(component), expected)?;
    }
    // ADR 0068 §3.4 — after manifest/CRC validation (so IO/Corruption
    // error classification is preserved for pre-existing failure
    // modes), probe each sub-archive for the ADR 0067 flag. Fail
    // closed on the legacy entry point BEFORE any staging directory
    // or destination exists.
    let mut any_bace_sub_archive = false;
    for component in COMPONENTS {
        let path = backup.join(component).join("backup.info");
        let receipt = backup::read_receipt(&path).map_err(|e| match e {
            StoreError::Corruption(msg) => DatabaseBackupError::Corruption(
                format!("{component} backup.info: {msg}"),
            ),
            other => DatabaseBackupError::Storage(other),
        })?;
        if receipt.flags.payload_has_bace_pages { any_bace_sub_archive = true; }
    }
    if any_bace_sub_archive && wiring.is_none() {
        return Err(DatabaseBackupError::RestoreRequiresBaceWiring);
    }
    let (destination, staged) = canonical_new_destination(destination, &backup)?;
    fs::create_dir(&staged)?;
    for (component, expected) in [
        (COMPONENTS[0], report.catalog),
        (COMPONENTS[1], report.domain),
        (COMPONENTS[2], report.lifecycle),
    ] {
        let actual = match &wiring {
            Some(w) => RowStore::restore_backup_with_bace(
                backup.join(component), staged.join(component),
                StorageLimits::default(), w.clone(),
            )?,
            None => RowStore::restore_backup(
                backup.join(component),
                staged.join(component),
            )?,
        };
        if !receipt_matches(actual, expected) {
            return Err(DatabaseBackupError::Corruption(
                format!("{component} backup receipt mismatch"),
            ));
        }
    }
    verify_reopened(&staged, &report, wiring.as_ref())?;
    sync_dir(&staged)?;
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(DatabaseBackupError::Invalid("destination appeared before restore publication".into()));
    }
    fs::rename(&staged, &destination)?;
    sync_dir(destination.parent().expect("canonical destination has parent"))?;
    Ok(report)
}
