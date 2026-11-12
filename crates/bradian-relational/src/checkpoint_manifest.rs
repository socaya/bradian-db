//! Atomic checkpoint generation publication for Bradian native row storage.
//!
//! A checkpoint file on disk is not automatically a committed generation.
//! The CRC-protected manifest is published only after snapshot and derived
//! index have been individually synced and their directory entries persisted.
//! Recovery reads the manifest *before* WAL replay and never guesses which
//! newer orphan snapshot is authoritative when a manifest exists.
//!
//! The format is deliberately fixed-size and host-endian independent.
//! File: checkpoint.current, staged as checkpoint.pending.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use crc32fast::Hasher;
use thiserror::Error;

const MAGIC: &[u8; 4] = b"BRCP";
const VERSION: u16 = 1;
const LENGTH: usize = 40;
const CURRENT: &str = "checkpoint.current";
const PENDING: &str = "checkpoint.pending";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckpointManifest {
    pub lsn: u64,
    pub rows: u64,
    pub data_pages: u64,
    pub index_pages: u64,
}

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid checkpoint manifest: {0}")]
    Corruption(&'static str),
    #[error("checkpoint generation would regress")]
    Regressed,
}

fn checksum(bytes: &[u8]) -> u32 {
    let mut h = Hasher::new();
    h.update(bytes);
    h.finalize()
}

impl CheckpointManifest {
    pub fn encode(self) -> [u8; LENGTH] {
        let mut bytes = [0_u8; LENGTH];
        bytes[0..4].copy_from_slice(MAGIC);
        bytes[4..6].copy_from_slice(&VERSION.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.lsn.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.rows.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.data_pages.to_le_bytes());
        bytes[32..36].copy_from_slice(&(self.index_pages as u32).to_le_bytes());
        let crc = checksum(&bytes[..36]);
        bytes[36..40].copy_from_slice(&crc.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() != LENGTH {
            return Err(ManifestError::Corruption("wrong manifest length"));
        }
        if &bytes[..4] != MAGIC || u16::from_le_bytes(bytes[4..6].try_into().unwrap()) != VERSION {
            return Err(ManifestError::Corruption("unsupported manifest magic or version"));
        }
        if bytes[6..8] != [0, 0] {
            return Err(ManifestError::Corruption("reserved manifest bytes are nonzero"));
        }
        let actual = u32::from_le_bytes(bytes[36..40].try_into().unwrap());
        if actual != checksum(&bytes[..36]) {
            return Err(ManifestError::Corruption("manifest checksum mismatch"));
        }
        Ok(Self {
            lsn: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            rows: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            data_pages: u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
            index_pages: u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as u64,
        })
    }
}

/// Return None only for a genuinely absent manifest. An invalid existing
/// manifest is never interpreted as a legacy database.
pub fn load(root: &Path) -> Result<Option<CheckpointManifest>, ManifestError> {
    let path = root.join(CURRENT);
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(ManifestError::Io(err)),
    };
    Ok(Some(CheckpointManifest::decode(&bytes)?))
}

/// Atomically replace the manifest. The caller must have already synced
/// both published snapshot/index files AND their directory entries. The WAL
/// may only be truncated *after* this function succeeds.
pub fn publish(root: &Path, next: CheckpointManifest) -> Result<(), ManifestError> {
    if next.index_pages > u32::MAX as u64 {
        return Err(ManifestError::Corruption("index page count cannot fit format"));
    }
    if let Some(current) = load(root)? {
        if next.lsn <= current.lsn {
            return Err(ManifestError::Regressed);
        }
    }
    let pending = root.join(PENDING);
    let mut file = OpenOptions::new().write(true).create(true).truncate(true).open(&pending)?;
    file.write_all(&next.encode())?;
    file.sync_all()?;
    drop(file);
    fs::rename(pending, root.join(CURRENT))?;
    sync_directory(root)?;
    Ok(())
}

/// A lightweight format check and length/bounds inspection before accepting
/// a manifest's published generation. Full BRPG CRC validation is performed
/// by RowStore's existing snapshot and index readers.
pub fn check_files(root: &Path, manifest: CheckpointManifest) -> Result<(), ManifestError> {
    let snapshot = root.join(format!("snapshot-{}.brp", manifest.lsn));
    let index = root.join(format!("index-{}.bri", manifest.lsn));
    let snapshot_len = fs::metadata(snapshot)?.len();
    let index_len = fs::metadata(index)?.len();
    let page_size = crate::page::PAGE_SIZE as u64;
    if snapshot_len != manifest.data_pages.checked_add(1)
        .and_then(|p| p.checked_mul(page_size))
        .ok_or(ManifestError::Corruption("snapshot page count overflow"))?
        || index_len != manifest.index_pages.checked_add(1)
        .and_then(|p| p.checked_mul(page_size))
        .ok_or(ManifestError::Corruption("index page count overflow"))?
    {
        return Err(ManifestError::Corruption("published file lengths disagree with manifest"));
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(root: &Path) -> std::io::Result<()> {
    File::open(root)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_root: &Path) -> std::io::Result<()> {
    Ok(())
}
