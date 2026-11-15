//! G24: independently verified single-node G23 historical database archives.
//!
//! This is a separate two-component backup format. G19's three-component
//! lifecycle/shared-WAL archives must never be interpreted as G24 archives.
//! Exclusive G11/G22 file locks are held through backup and verification.
//! Archives preserve retained versions, tombstones and the retention floor.

use crate::runtime_limits::HistoryLimits;
use crate::runtime_limits::MetadataLimits;
use crate::metadata_io;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crc32fast::Hasher;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Table, TableId};
use crate::backup::{self, BackupReceipt};
use crate::durable_typed::DurableTypedStore;
use crate::persistent_catalog::{PersistentCatalog, PersistentCatalogError};
use crate::row_store::{BaceWiring, RowStore, StoreError};
use crate::runtime_limits::StorageLimits;
use crate::transaction_domain::DomainError;

const FORMAT: &str = "BRADIAN-HISTORY-BACKUP-V1";
const MANIFEST: &str = "history-backup.json";
const COMPONENTS: [&str; 2] = ["catalog", "history-domain"];

#[derive(Debug, Error)]
pub enum HistoryBackupError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Catalog(#[from] PersistentCatalogError),
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("history backup refused: {0}")]
    Invalid(String),
    #[error("history backup verification failed: {0}")]
    Corruption(String),
    #[error("history backup carries BACE v2 pages in one or more components; restore_history_database_with_bace is required")]
    RestoreRequiresBaceWiring,
}
pub type HistoryBackupResult<T> = Result<T, HistoryBackupError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryComponentReceipt {
    pub checkpoint_lsn: u64,
    pub last_lsn: u64,
    pub rows: u64,
    pub archive_crc32: u32,
}
impl From<BackupReceipt> for HistoryComponentReceipt {
    fn from(value: BackupReceipt) -> Self {
        Self {
            checkpoint_lsn: value.checkpoint_lsn,
            last_lsn: value.last_lsn,
            rows: value.rows,
            archive_crc32: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryBackupReport {
    pub catalog: HistoryComponentReceipt,
    pub history: HistoryComponentReceipt,
    pub table_ids: Vec<u64>,
    pub retention_floor: u64,
    pub retained_versions: usize,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    format: String,
    report: HistoryBackupReport,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    payload: Payload,
    crc32: u32,
}

fn sync_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    { File::open(path)?.sync_all() }
    #[cfg(not(unix))]
    { let _ = path; Ok(()) }
}

fn new_destination(dest: &Path, forbidden: &Path) -> HistoryBackupResult<(PathBuf, PathBuf)> {
    let parent = fs::canonicalize(dest.parent().filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new(".")))?;
    let name = dest.file_name().ok_or_else(||
        HistoryBackupError::Invalid("destination name missing".into()))?;
    let dest = parent.join(name);
    let excluded = fs::canonicalize(forbidden)?;
    let stage = parent.join(format!(".{}.bradian-g24-incomplete", name.to_string_lossy()));
    for candidate in [&dest, &stage] {
        if candidate.starts_with(&excluded) || excluded.starts_with(candidate) {
            return Err(HistoryBackupError::Invalid("source and destination overlap".into()));
        }
        if fs::symlink_metadata(candidate).is_ok() {
            return Err(HistoryBackupError::Invalid(format!(
                "destination or unfinished stage exists: {}", candidate.display()
            )));
        }
    }
    Ok((dest, stage))
}

fn archive_crc(root: &Path, receipt: HistoryComponentReceipt) -> HistoryBackupResult<u32> {
    let files = [
        "backup.info".to_string(),
        "checkpoint.current".to_string(),
        format!("snapshot-{}.brp", receipt.checkpoint_lsn),
        format!("index-{}.bri", receipt.checkpoint_lsn),
        "rows.wal".to_string(),
    ];
    let mut digest = Hasher::new();
    for name in files {
        digest.update(&(name.len() as u32).to_le_bytes());
        digest.update(name.as_bytes());
        let mut file = File::open(root.join(name))?;
        let mut chunk = [0_u8; 65536];
        loop {
            let n = file.read(&mut chunk)?;
            if n == 0 { break; }
            digest.update(&chunk[..n]);
        }
    }
    Ok(digest.finalize())
}

#[derive(Serialize)]
struct PayloadRef<'a> { format: &'static str, report: &'a HistoryBackupReport }
#[derive(Serialize)]
struct EnvelopeRef<'a> { payload: PayloadRef<'a>, crc32: u32 }
fn write_manifest(root: &Path, report: &HistoryBackupReport, limits: MetadataLimits) -> Result<(),HistoryBackupError> {
    let payload=PayloadRef { format:FORMAT,report };
    let crc32=metadata_io::json_crc(&payload,limits)?;
    let bytes=metadata_io::json_bytes(&EnvelopeRef { payload,crc32 },limits)?;
    let mut file=OpenOptions::new().write(true).create_new(true).open(root.join(MANIFEST))?;
    file.write_all(&bytes)?; file.sync_all()?; sync_dir(root)?; Ok(())
}

fn read_manifest(root: &Path, limits: MetadataLimits) -> HistoryBackupResult<HistoryBackupReport> {
    let bytes=metadata_io::read_json_bytes(&root.join(MANIFEST),limits).map_err(|error|match error {
        StoreError::Io(error)=>HistoryBackupError::Io(error), other=>HistoryBackupError::Store(other),
    })?;
    let envelope: Envelope = serde_json::from_slice(&bytes)
        .map_err(|e| HistoryBackupError::Corruption(format!("invalid manifest: {e}")))?;
    if envelope.payload.format != FORMAT
        || metadata_io::json_crc(&envelope.payload,limits)? != envelope.crc32
    {
        return Err(HistoryBackupError::Corruption("manifest format or CRC invalid".into()));
    }
    let report = envelope.payload.report;
    if report.table_ids.is_empty() ||
        report.table_ids.windows(2).any(|w| w[0] >= w[1]) ||
        report.catalog.last_lsn < report.catalog.checkpoint_lsn ||
        report.history.last_lsn < report.history.checkpoint_lsn ||
        report.retention_floor > report.history.last_lsn
    {
        return Err(HistoryBackupError::Corruption("manifest identities or LSNs invalid".into()));
    }
    Ok(report)
}

fn catalog_tables(catalog: &PersistentCatalog) -> BTreeMap<TableId, Table> {
    catalog.snapshot().tables().map(|t| (t.id, t.clone())).collect()
}

fn verify_root(
    root: &Path, expected: &HistoryBackupReport, limits: HistoryLimits,
    bace: Option<&BaceWiring>,
) -> HistoryBackupResult<()> {
    // This check also rejects legacy ownership and unregistered historical rows.
    if root.join("shared-domain").exists() || root.join("lifecycle").exists() ||
        root.join("tables").exists()
    {
        return Err(HistoryBackupError::Corruption("mixed legacy storage root".into()));
    }
    let catalog = match bace {
        Some(w) => PersistentCatalog::open_with_bace(root.join("catalog"), w.clone())?,
        None => PersistentCatalog::open(root.join("catalog"))?,
    };
    let definitions = catalog_tables(&catalog);
    let domain = match bace {
        Some(w) => DurableTypedStore::open_with_bace_and_history_limits(
            root.join("history-domain"), &definitions, w.clone(), limits,
        )?,
        None => DurableTypedStore::open_with_history_limits(
            root.join("history-domain"), &definitions, limits,
        )?,
    };
    domain.verify_ddl_catalog_horizon(catalog.last_lsn())?;
    let stats = domain.stats();
    if catalog.last_lsn() != expected.catalog.last_lsn ||
        catalog.journal_row_count() != expected.catalog.rows ||
        domain.last_lsn() != expected.history.last_lsn ||
        domain.active_tables().iter().map(|id| id.0).collect::<Vec<_>>() != expected.table_ids ||
        stats.retention_floor != expected.retention_floor ||
        stats.persisted_versions != expected.retained_versions
    {
        return Err(HistoryBackupError::Corruption(
            "restored catalog/history identities, LSNs or retained versions differ".into()
        ));
    }
    Ok(())
}

fn receipt_matches(actual: BackupReceipt, expected: HistoryComponentReceipt) -> bool {
    actual.checkpoint_lsn == expected.checkpoint_lsn &&
        actual.last_lsn == expected.last_lsn && actual.rows == expected.rows
}

/// Source holders must maintain both independent file locks throughout this call.
/// Schema DDL requires mutable engine access and cannot overlap backup. G22
/// receipt statistics describe the archived snapshot even as row writers resume.
#[allow(clippy::too_many_arguments)]
pub(crate) fn backup_history_database(
    source: &Path,
    destination: &Path,
    catalog: &PersistentCatalog,
    history: &DurableTypedStore,
    metadata: MetadataLimits,
    bace: Option<&BaceWiring>,
) -> HistoryBackupResult<HistoryBackupReport> {
    if source.join("lifecycle").exists() || source.join("shared-domain").exists()
        || source.join("tables").exists()
    {
        return Err(HistoryBackupError::Invalid("mixed legacy storage source".into()));
    }
    let (dest, stage) = new_destination(destination, source)?;
    fs::create_dir(&stage)?;
    let mut catalog_point = HistoryComponentReceipt::from(
        catalog.backup_catalog_to(stage.join(COMPONENTS[0]))?);
    let (history_receipt, stats) = history.backup_with_stats(stage.join(COMPONENTS[1]))?;
    let mut history_point = HistoryComponentReceipt::from(history_receipt);
    catalog_point.archive_crc32 = archive_crc(&stage.join(COMPONENTS[0]), catalog_point)?;
    history_point.archive_crc32 = archive_crc(&stage.join(COMPONENTS[1]), history_point)?;
    let report = HistoryBackupReport {
        catalog: catalog_point,
        history: history_point,
        table_ids: history.active_tables().iter().map(|id| id.0).collect(),
        retention_floor: stats.retention_floor,
        retained_versions: stats.persisted_versions,
    };
    write_manifest(&stage, &report, metadata)?;
    let verify = stage.join("verified-root");
    fs::create_dir(&verify)?;
    // ADR 0070 — when the G24 engine was opened with BACE wiring, the
    // sub-archives carry BACE v2 pages (flag set on each backup.info);
    // restore via _with_bace variants.
    for component in COMPONENTS {
        match bace {
            Some(w) => {
                RowStore::restore_backup_with_bace(
                    stage.join(component), verify.join(component),
                    StorageLimits::default(), w.clone(),
                )?;
            }
            None => {
                RowStore::restore_backup(stage.join(component), verify.join(component))?;
            }
        }
    }
    verify_root(&verify, &report, history.history_limits(), bace)?;
    fs::remove_dir_all(&verify)?;
    sync_dir(&stage)?;
    if fs::symlink_metadata(&dest).is_ok() {
        return Err(HistoryBackupError::Invalid("destination appeared before publish".into()));
    }
    fs::rename(&stage, &dest)?;
    sync_dir(dest.parent().expect("canonical destination has parent"))?;
    Ok(report)
}

/// Restore a complete two-component historical archive to an absent root.
/// The G10 archives and reopened G23 schema/history are verified BEFORE rename.
pub fn restore_history_database(
    backup: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> HistoryBackupResult<HistoryBackupReport> {
    restore_history_database_with_limits(backup, destination, HistoryLimits::default())
}

pub fn restore_history_database_with_limits(
    backup: impl AsRef<Path>, destination: impl AsRef<Path>, limits: HistoryLimits,
) -> HistoryBackupResult<HistoryBackupReport> {
    restore_history_database_with_policies(backup,destination,limits,MetadataLimits::default())
}

pub fn restore_history_database_with_policies(
    backup: impl AsRef<Path>, destination: impl AsRef<Path>, limits: HistoryLimits, metadata: MetadataLimits,
) -> HistoryBackupResult<HistoryBackupReport> {
    restore_history_database_inner(backup.as_ref(), destination.as_ref(), limits, metadata, None)
}

/// ADR 0068 §3.1 — BACE-aware history coordinator restore. Required
/// when any sub-archive's `backup.info` declares BACE pages.
/// Tolerated on legacy (flag-clear) coordinator backups.
pub fn restore_history_database_with_bace(
    backup: impl AsRef<Path>,
    destination: impl AsRef<Path>,
    wiring: BaceWiring,
) -> HistoryBackupResult<HistoryBackupReport> {
    restore_history_database_with_bace_and_policies(
        backup, destination, HistoryLimits::default(), MetadataLimits::default(), wiring,
    )
}

pub fn restore_history_database_with_bace_and_policies(
    backup: impl AsRef<Path>,
    destination: impl AsRef<Path>,
    limits: HistoryLimits,
    metadata: MetadataLimits,
    wiring: BaceWiring,
) -> HistoryBackupResult<HistoryBackupReport> {
    restore_history_database_inner(
        backup.as_ref(), destination.as_ref(), limits, metadata, Some(wiring),
    )
}

fn restore_history_database_inner(
    backup: &Path,
    destination: &Path,
    limits: HistoryLimits,
    metadata: MetadataLimits,
    wiring: Option<BaceWiring>,
) -> HistoryBackupResult<HistoryBackupReport> {
    let backup = fs::canonicalize(backup)?;
    let report = read_manifest(&backup,metadata)?;
    for (component, receipt) in [
        (COMPONENTS[0], report.catalog), (COMPONENTS[1], report.history),
    ] {
        if archive_crc(&backup.join(component), receipt)? != receipt.archive_crc32 {
            return Err(HistoryBackupError::Corruption(format!("{component} checksum mismatch")));
        }
    }
    // ADR 0068 §3.4 — after manifest/CRC validation, probe each
    // sub-archive for the ADR 0067 flag and fail closed on the
    // legacy entry point BEFORE any staging directory exists.
    let mut any_bace_sub_archive = false;
    for component in COMPONENTS {
        let path = backup.join(component).join("backup.info");
        let receipt = backup::read_receipt(&path).map_err(|e| match e {
            StoreError::Corruption(msg) => HistoryBackupError::Corruption(
                format!("{component} backup.info: {msg}"),
            ),
            other => HistoryBackupError::Store(other),
        })?;
        if receipt.flags.payload_has_bace_pages { any_bace_sub_archive = true; }
    }
    if any_bace_sub_archive && wiring.is_none() {
        return Err(HistoryBackupError::RestoreRequiresBaceWiring);
    }
    let (dest, stage) = new_destination(destination, &backup)?;
    fs::create_dir(&stage)?;
    for (component, receipt) in [
        (COMPONENTS[0], report.catalog), (COMPONENTS[1], report.history),
    ] {
        let actual = match &wiring {
            Some(w) => RowStore::restore_backup_with_bace(
                backup.join(component), stage.join(component),
                StorageLimits::default(), w.clone(),
            )?,
            None => RowStore::restore_backup(backup.join(component), stage.join(component))?,
        };
        if !receipt_matches(actual, receipt) {
            return Err(HistoryBackupError::Corruption(format!("{component} receipt mismatch")));
        }
    }
    verify_root(&stage, &report, limits, wiring.as_ref())?;
    sync_dir(&stage)?;
    if fs::symlink_metadata(&dest).is_ok() {
        return Err(HistoryBackupError::Invalid("destination appeared before restore publish".into()));
    }
    fs::rename(&stage, &dest)?;
    sync_dir(dest.parent().expect("canonical destination has parent"))?;
    Ok(report)
}
