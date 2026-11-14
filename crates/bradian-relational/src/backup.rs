//! Consistent, verifiable offline backup and restore for Bradian RowStore.
//!
//! A backup captures one committed BRCP checkpoint plus the WAL containing
//! subsequent acknowledged mutations. RowStore holds its mutation mutex while
//! copying the files, ensuring they form a single recoverable LSN boundary.
//!
//! Only exact authoritative files are included. The destination is prepared
//! in a sibling staging directory and published by rename after the copied
//! database has been reopened and its physical index verified. Restore works
//! only into a new directory and never overwrites a running database.
//!
//! This is a local-filesystem durability facility, not encrypted backup,
//! remote object-storage snapshotting, incremental PITR, or a cluster backup.

use crate::runtime_limits::{RowStoreLimits, IndexLimits, StorageLimits};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use crc32fast::hash;

use crate::checkpoint_manifest;
use crate::row_store::{BaceWiring, RowStore, StoreError, StoreResult};

const MAGIC: &[u8; 8] = b"BRBKv001";
const META_FILE: &str = "backup.info";
const DESCRIPTOR_LENGTH: usize = 40;

/// ADR 0067 §3.2 — backup descriptor flags, stored as u32 LE at
/// bytes 32..36 of the 40-byte on-disk descriptor. Legacy
/// (pre-ADR-0067) descriptors have these bytes zero and decode to
/// a default `BackupFlags { payload_has_bace_pages: false }`.
///
/// One flag bit allocated: `FLAG_PAYLOAD_HAS_BACE_PAGES` (bit 0).
/// Remaining 31 bits are reserved and MUST decode as zero;
/// non-zero reserved bits fail closed with `Corruption`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BackupFlags {
    /// The backup's copied snapshot carries BRPG v2 pages wrapping
    /// BACE envelopes. Restore must use `restore_backup_with_bace`.
    pub payload_has_bace_pages: bool,
}

const FLAG_PAYLOAD_HAS_BACE_PAGES: u32 = 0x0000_0001;
const FLAG_KNOWN_MASK: u32 = FLAG_PAYLOAD_HAS_BACE_PAGES;

impl BackupFlags {
    fn to_u32(self) -> u32 {
        let mut bits = 0u32;
        if self.payload_has_bace_pages { bits |= FLAG_PAYLOAD_HAS_BACE_PAGES; }
        bits
    }

    fn from_u32(bits: u32) -> StoreResult<Self> {
        if bits & !FLAG_KNOWN_MASK != 0 {
            return Err(StoreError::Corruption(
                "reserved backup flag bits non-zero".into(),
            ));
        }
        Ok(Self {
            payload_has_bace_pages: bits & FLAG_PAYLOAD_HAS_BACE_PAGES != 0,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupReceipt {
    pub checkpoint_lsn: u64,
    pub last_lsn: u64,
    pub rows: u64,
    pub flags: BackupFlags,
}

impl BackupReceipt {
    pub fn encode(self) -> [u8; DESCRIPTOR_LENGTH] {
        let mut bytes = [0_u8; DESCRIPTOR_LENGTH];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8..16].copy_from_slice(&self.checkpoint_lsn.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.last_lsn.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.rows.to_le_bytes());
        bytes[32..36].copy_from_slice(&self.flags.to_u32().to_le_bytes());
        let checksum = hash(&bytes[..36]);
        bytes[36..40].copy_from_slice(&checksum.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> StoreResult<Self> {
        if bytes.len() != DESCRIPTOR_LENGTH || &bytes[..8] != MAGIC {
            return Err(StoreError::Corruption("invalid backup descriptor header".into()));
        }
        if u32::from_le_bytes(bytes[36..40].try_into().unwrap()) != hash(&bytes[..36]) {
            return Err(StoreError::Corruption("backup descriptor checksum mismatch".into()));
        }
        let flag_bits = u32::from_le_bytes(bytes[32..36].try_into().unwrap());
        let flags = BackupFlags::from_u32(flag_bits)?;
        let receipt = Self {
            checkpoint_lsn: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            last_lsn: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            rows: u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
            flags,
        };
        if receipt.last_lsn < receipt.checkpoint_lsn {
            return Err(StoreError::Corruption("backup LSN precedes checkpoint".into()));
        }
        Ok(receipt)
    }
}

/// ADR 0068 — exposes the on-disk `backup.info` descriptor so
/// coordinator backups (database_backup.rs, history_backup.rs) can
/// probe each sub-archive's `BackupFlags` without reopening the
/// RowStore.
pub fn read_receipt(path:&Path)->StoreResult<BackupReceipt> {
    let bytes=crate::metadata_io::read_fixed::<DESCRIPTOR_LENGTH>(path).map_err(|e|match e {
        crate::metadata_io::FixedError::Io(e)=>StoreError::Io(e),
        crate::metadata_io::FixedError::Length=>StoreError::Corruption("invalid backup descriptor header".into()),
    })?;
    BackupReceipt::decode(&bytes)
}

/// Reject potentially dangerous overlap and return a sibling staging path.
/// Neither a destination nor an old incomplete staging directory is replaced.
fn staging_path(destination: &Path, forbidden_root: Option<&Path>) -> StoreResult<PathBuf> {
    let name = destination.file_name()
        .ok_or_else(|| StoreError::Corruption("backup destination requires a directory name".into()))?;
    let parent = destination.parent().filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let actual_parent = fs::canonicalize(parent)?;
    if let Some(forbidden) = forbidden_root {
        if actual_parent.starts_with(fs::canonicalize(forbidden)?) {
            return Err(StoreError::Corruption("backup destination must be outside the active database directory".into()));
        }
    }
    if fs::symlink_metadata(destination).is_ok() {
        return Err(StoreError::Corruption("destination already exists".into()));
    }
    let staged = actual_parent.join(format!(".{}.bradian-incomplete", name.to_string_lossy()));
    if fs::symlink_metadata(&staged).is_ok() {
        return Err(StoreError::Corruption("incomplete staging directory already exists".into()));
    }
    Ok(staged)
}

fn copy_sync(source: &Path, target: &Path) -> StoreResult<()> {
    let mut from = File::open(source)?;
    let mut to = fs::OpenOptions::new().write(true).create_new(true).open(target)?;
    std::io::copy(&mut from, &mut to)?;
    to.sync_all()?;
    Ok(())
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn payload_names(checkpoint_lsn: u64) -> [String; 4] {
    [
        "checkpoint.current".into(),
        format!("snapshot-{checkpoint_lsn}.brp"),
        format!("index-{checkpoint_lsn}.bri"),
        "rows.wal".into(),
    ]
}

/// Copy the immutable checkpoint and synchronized WAL while the caller holds
/// the RowStore mutation mutex. The source is never reopened or mutated here.
///
/// `bace`: if the source's checkpoint contains BRPG v2 pages (ADR 0065),
/// the caller MUST pass the BACE wiring so staging-side `verify_directory`
/// can reopen the copied directory through `RowStore::open_with_bace`.
/// The receipt's `flags.payload_has_bace_pages` must agree with
/// `bace.is_some() && non-raw profile` — the caller sets this.
pub(crate) fn create_backup(
    root: &Path,
    destination: &Path,
    receipt: BackupReceipt,
    expected_rows: &BTreeMap<Vec<u8>, Vec<u8>>,
    policies: StorageLimits,
    bace: Option<&BaceWiring>,
) -> StoreResult<BackupReceipt> {
    let manifest = checkpoint_manifest::load(root)?
        .ok_or_else(|| StoreError::Corruption("backup requires a committed checkpoint manifest".into()))?;
    if manifest.lsn != receipt.checkpoint_lsn {
        return Err(StoreError::Corruption("checkpoint changed during backup".into()));
    }
    checkpoint_manifest::check_snapshot(root, manifest)?;
    checkpoint_manifest::check_index(root, manifest)?;
    let staged = staging_path(destination, Some(root))?;
    fs::create_dir(&staged)?;
    for file in payload_names(receipt.checkpoint_lsn) {
        copy_sync(&root.join(&file), &staged.join(&file))?;
    }
    let mut metadata = fs::OpenOptions::new().write(true).create_new(true)
        .open(staged.join(META_FILE))?;
    metadata.write_all(&receipt.encode())?;
    metadata.sync_all()?;
    sync_directory(&staged)?;
    // Validate the copied database independently: page CRCs, WAL sequencing,
    // index row pointers, and the exact acknowledged row state and LSN.
    verify_directory(&staged, receipt, Some(expected_rows), policies, bace)?;
    fs::rename(&staged, destination)?;
    let parent = destination.parent().filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    sync_directory(parent)?;
    Ok(receipt)
}

fn verify_directory(
    path: &Path,
    receipt: BackupReceipt,
    expected_rows: Option<&BTreeMap<Vec<u8>, Vec<u8>>>,
    policies: StorageLimits,
    bace: Option<&BaceWiring>,
) -> StoreResult<()> {
    let stored = read_receipt(&path.join(META_FILE))?;
    if stored != receipt {
        return Err(StoreError::Corruption("backup descriptor differs from intended state".into()));
    }
    let manifest = checkpoint_manifest::load(path)?
        .ok_or_else(|| StoreError::Corruption("backup missing checkpoint manifest".into()))?;
    if manifest.lsn != receipt.checkpoint_lsn {
        return Err(StoreError::Corruption("backup checkpoint generation mismatch".into()));
    }
    // RowStore's ordinary crash recovery may trim an incomplete WAL tail.
    // A published backup must be exact: never silently repair its copied WAL.
    let wal_path = path.join("rows.wal");
    let original_wal_len = fs::metadata(&wal_path)?.len();
    // ADR 0067 §3.5 — route through open_with_bace when the receipt
    // says the copied snapshot has BACE v2 pages. Also route through
    // open_with_bace when the caller passed wiring even on a v1-only
    // directory (ADR 0065 §3.5 mixed-generation rule allows that).
    let recovered = match bace {
        Some(w) => RowStore::open_with_bace(path, policies, w.clone())?,
        None    => RowStore::open_with_policies(path, policies)?,
    };
    if fs::metadata(&wal_path)?.len() != original_wal_len {
        return Err(StoreError::Corruption(
            "backup WAL required tail repair during validation".into(),
        ));
    }
    if recovered.last_lsn() != receipt.last_lsn || recovered.len() as u64 != receipt.rows {
        return Err(StoreError::Corruption("backup replay does not match committed LSN or row count".into()));
    }
    recovered.verify_checkpoint_index()?;
    if let Some(expected) = expected_rows {
        if recovered.len() != expected.len() {
            return Err(StoreError::Corruption("backed-up row count does not match the source".into()));
        }
        recovered.visit_snapshot(|_,key,value| -> StoreResult<()> {
            if expected.get(key).is_none_or(|expected| expected.as_slice() != value) {
                return Err(StoreError::Corruption("backed-up row values do not match the source".into()));
            }
            Ok(())
        })?;
    }
    Ok(())
}

/// Copy a committed Bradian backup into a new destination, validate every
/// authoritative file, replay the WAL, audit the derived physical index, then
/// atomically publish the restored directory.
///
/// This operation intentionally does not open or mutate the original backup.
pub fn restore_backup(backup: &Path, destination: &Path) -> StoreResult<BackupReceipt> {
    restore_backup_with_limits(backup, destination, RowStoreLimits::default())
}

pub fn restore_backup_with_limits(backup: &Path, destination: &Path, limits: RowStoreLimits) -> StoreResult<BackupReceipt> {
    restore_backup_with_recovery_limits(backup, destination, limits, IndexLimits::default())
}

pub fn restore_backup_with_recovery_limits(backup: &Path, destination: &Path, limits: RowStoreLimits, index_limits: IndexLimits) -> StoreResult<BackupReceipt> {
    restore_backup_with_policies(backup, destination, StorageLimits { rows:limits,index:index_limits,..StorageLimits::default() })
}

pub fn restore_backup_with_policies(backup: &Path, destination: &Path, policies: StorageLimits) -> StoreResult<BackupReceipt> {
    restore_backup_inner(backup, destination, policies, None)
}

/// ADR 0067 §3.4 — BACE-aware restore. Required for backups whose
/// receipt has `flags.payload_has_bace_pages` set. Tolerated on
/// legacy (flag-clear) backups: ADR 0065 §3.5 guarantees
/// `open_with_bace` works on a v1-only directory.
pub fn restore_backup_with_bace(
    backup: &Path,
    destination: &Path,
    policies: StorageLimits,
    wiring: BaceWiring,
) -> StoreResult<BackupReceipt> {
    restore_backup_inner(backup, destination, policies, Some(wiring))
}

fn restore_backup_inner(
    backup: &Path,
    destination: &Path,
    policies: StorageLimits,
    wiring: Option<BaceWiring>,
) -> StoreResult<BackupReceipt> {
    if !backup.is_dir() {
        return Err(StoreError::Corruption("backup source is not a directory".into()));
    }
    let receipt = read_receipt(&backup.join(META_FILE))?;
    // ADR 0067 §3.4 — reject BACE backups on the legacy path BEFORE
    // creating any staging directory. Operators see exactly which
    // entry point to use; nothing on disk is touched.
    if receipt.flags.payload_has_bace_pages && wiring.is_none() {
        return Err(StoreError::BackupRestoreRequiresBaceWiring);
    }
    let staged = staging_path(destination, Some(backup))?;
    // Do not restore into the backup itself or overlap its parent tree.
    if fs::canonicalize(backup)?.starts_with(&staged) {
        return Err(StoreError::Corruption("restore destination overlaps backup".into()));
    }
    fs::create_dir(&staged)?;
    for name in payload_names(receipt.checkpoint_lsn) {
        copy_sync(&backup.join(&name), &staged.join(&name))?;
    }
    copy_sync(&backup.join(META_FILE), &staged.join(META_FILE))?;
    sync_directory(&staged)?;
    verify_directory(&staged, receipt, None, policies, wiring.as_ref())?;
    fs::rename(&staged, destination)?;
    let parent = destination.parent().filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    sync_directory(parent)?;
    Ok(receipt)
}
