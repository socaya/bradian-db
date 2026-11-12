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

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crc32fast::{hash, Hasher};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::backup::BackupReceipt;
use crate::lifecycle::{
    CatalogLifecycle, LifecycleError, TableLifecycleState, TableLifecycleStatus,
};
use crate::persistent_catalog::{PersistentCatalog, PersistentCatalogError};
use crate::row_store::{RowStore, StoreError};
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

fn write_manifest(root: &Path, report: &DatabaseBackupReport) -> DatabaseBackupResult<()> {
    let payload = ManifestPayload { format: FORMAT.into(), report: report.clone() };
    let payload_bytes = serde_json::to_vec(&payload)?;
    let envelope = Envelope {
        payload,
        crc32: hash(&payload_bytes),
    };
    let mut file = OpenOptions::new().write(true).create_new(true)
        .open(root.join(MANIFEST))?;
    file.write_all(&serde_json::to_vec(&envelope)?)?;
    file.sync_all()?;
    sync_dir(root)?;
    Ok(())
}

fn read_manifest(root: &Path) -> DatabaseBackupResult<DatabaseBackupReport> {
    let envelope: Envelope = serde_json::from_slice(&fs::read(root.join(MANIFEST))?)
        .map_err(|e| DatabaseBackupError::Corruption(format!("manifest JSON: {e}")))?;
    if envelope.payload.format != FORMAT {
        return Err(DatabaseBackupError::Corruption("unsupported database backup version".into()));
    }
    let payload_bytes = serde_json::to_vec(&envelope.payload)?;
    if hash(&payload_bytes) != envelope.crc32 {
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
) -> DatabaseBackupResult<()> {
    let owner = CatalogLifecycle::open(root)?;
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
pub(crate) fn backup_database(
    source_root: &Path,
    destination: &Path,
    catalog: &PersistentCatalog,
    domain: &MultiTableStore,
    lifecycle: &RowStore,
    statuses: &[TableLifecycleStatus],
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
    write_manifest(&staged, &report)?;
    // Rebuild, replay and compare the actual relational catalog, physical
    // domain schema pins and lifecycle journal in the independently copied
    // backup before publishing it.
    // A G10 component backup is itself a directory of RowStore files, not a
    // live database root. Restore each one into an additional temporary root.
    let verify = staged.join("verified-root");
    fs::create_dir(&verify)?;
    for component in COMPONENTS {
        RowStore::restore_backup(staged.join(component), verify.join(component))?;
    }
    verify_reopened(&verify, &report)?;
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
    let backup = fs::canonicalize(backup.as_ref())?;
    if !backup.is_dir() {
        return Err(DatabaseBackupError::Invalid("backup is not a directory".into()));
    }
    let report = read_manifest(&backup)?;
    for (component, expected) in [
        (COMPONENTS[0], report.catalog),
        (COMPONENTS[1], report.domain),
        (COMPONENTS[2], report.lifecycle),
    ] {
        verify_archive_crc(&backup.join(component), expected)?;
    }
    let (destination, staged) = canonical_new_destination(destination.as_ref(), &backup)?;
    fs::create_dir(&staged)?;
    for (component, expected) in [
        (COMPONENTS[0], report.catalog),
        (COMPONENTS[1], report.domain),
        (COMPONENTS[2], report.lifecycle),
    ] {
        let actual = RowStore::restore_backup(
            backup.join(component),
            staged.join(component),
        )?;
        if !receipt_matches(actual, expected) {
            return Err(DatabaseBackupError::Corruption(
                format!("{component} backup receipt mismatch"),
            ));
        }
    }
    verify_reopened(&staged, &report)?;
    sync_dir(&staged)?;
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(DatabaseBackupError::Invalid("destination appeared before restore publication".into()));
    }
    fs::rename(&staged, &destination)?;
    sync_dir(destination.parent().expect("canonical destination has parent"))?;
    Ok(report)
}
