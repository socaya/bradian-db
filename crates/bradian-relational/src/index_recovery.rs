//! Reconstruct a derived B+tree from authoritative BRPG checkpoint pages.
//!
//! The checkpoint snapshot and synchronized WAL—not the index—are the source
//! of truth. This module never changes row or WAL data. A missing index can
//! be recovered by building a new immutable generation using verified row
//! pointers. An existing index is never overwritten implicitly.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use crate::btree::{DiskBTree, LeafEntries, RowPointer};
use crate::page::{Page, PageKind, PAGE_SIZE};
use crate::row_store::{StoreError, StoreResult};

type VerifiedSnapshot = (LeafEntries, BTreeMap<Vec<u8>, Vec<u8>>, u64);

const ROW_HEADER: usize = 6;
const META_LENGTH: usize = 24;
const MAX_KEY: usize = 512;
const MAX_VALUE: usize = 3546;

/// Index reconstruction report, based on validated authoritative snapshot pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexRecovery {
    pub checkpoint_lsn: u64,
    pub rows: u64,
    pub data_pages: u64,
    pub rebuilt: bool,
}

pub fn index_path(root: &Path, lsn: u64) -> std::path::PathBuf {
    root.join(format!("index-{lsn}.bri"))
}

fn snapshot_path(root: &Path, lsn: u64) -> std::path::PathBuf {
    root.join(format!("snapshot-{lsn}.brp"))
}

/// Read all physical row pointers, strictly validating the authoritative page
/// stream first. The returned key/value map is used for consistency checks;
/// nothing from the existing derived index is trusted.
fn scan_snapshot(
    root: &Path,
    lsn: u64,
) -> StoreResult<VerifiedSnapshot> {
    let mut file = File::open(snapshot_path(root, lsn))?;
    let length = file.metadata()?.len();
    if length < PAGE_SIZE as u64 || length % PAGE_SIZE as u64 != 0 {
        return Err(StoreError::Corruption("invalid snapshot file length".into()));
    }
    let mut bytes = [0_u8; PAGE_SIZE];
    file.read_exact(&mut bytes)?;
    let header = Page::decode(&bytes)?;
    if header.kind != PageKind::Metadata || header.page_id != 0
        || header.lsn != lsn || header.payload.len() != META_LENGTH
    {
        return Err(StoreError::Corruption("snapshot metadata page is invalid".into()));
    }
    let recorded_lsn = u64::from_le_bytes(header.payload[0..8].try_into().unwrap());
    let expected_rows = u64::from_le_bytes(header.payload[8..16].try_into().unwrap());
    let data_pages = u64::from_le_bytes(header.payload[16..24].try_into().unwrap());
    if recorded_lsn != lsn ||
        data_pages.checked_add(1).and_then(|count| count.checked_mul(PAGE_SIZE as u64))
            != Some(length)
    {
        return Err(StoreError::Corruption("snapshot generation or physical page count mismatch".into()));
    }

    let mut keys = Vec::new();
    let mut records = BTreeMap::new();
    let mut last: Option<Vec<u8>> = None;
    for page_id in 1..=data_pages {
        file.read_exact(&mut bytes)?;
        let page = Page::decode(&bytes)?;
        if page.kind != PageKind::Data || page.page_id != page_id || page.lsn != lsn {
            return Err(StoreError::Corruption("snapshot data page identity mismatch".into()));
        }
        let mut offset = 0_usize;
        while offset < page.payload.len() {
            let remaining = page.payload.len() - offset;
            if remaining < ROW_HEADER {
                return Err(StoreError::Corruption("truncated snapshot row header".into()));
            }
            let key_len = u16::from_le_bytes(
                page.payload[offset..offset + 2].try_into().unwrap()
            ) as usize;
            let value_len = u32::from_le_bytes(
                page.payload[offset + 2..offset + 6].try_into().unwrap()
            ) as usize;
            let record_len = ROW_HEADER.checked_add(key_len)
                .and_then(|n| n.checked_add(value_len))
                .ok_or_else(|| StoreError::Corruption("snapshot record length overflow".into()))?;
            if key_len == 0 || key_len > MAX_KEY || value_len > MAX_VALUE ||
                record_len > remaining
            {
                return Err(StoreError::Corruption("snapshot row record bounds invalid".into()));
            }
            let key_start = offset + ROW_HEADER;
            let key_end = key_start + key_len;
            let key = page.payload[key_start..key_end].to_vec();
            let value = page.payload[key_end..offset + record_len].to_vec();
            if last.as_ref().is_some_and(|prev| prev >= &key) {
                return Err(StoreError::Corruption("snapshot row order is not strictly increasing".into()));
            }
            let pointer = RowPointer {
                page_id,
                offset: u16::try_from(offset)
                    .map_err(|_| StoreError::Corruption("row pointer offset overflow".into()))?,
            };
            keys.push((key.clone(), pointer));
            records.insert(key.clone(), value);
            last = Some(key);
            offset += record_len;
        }
    }
    if keys.len() as u64 != expected_rows {
        return Err(StoreError::Corruption("snapshot row count differs from metadata".into()));
    }
    Ok((keys, records, data_pages))
}

/// Verify an existing index against every row pointer from its authoritative
/// checkpoint. This is O(N log N) and intended for an explicit integrity audit,
/// not every normal query.
pub fn verify(root: &Path, lsn: u64) -> StoreResult<IndexRecovery> {
    let (entries, _rows, pages) = scan_snapshot(root, lsn)?;
    let mut index = DiskBTree::open(&index_path(root, lsn), lsn)?;
    if index.len() != entries.len() as u64 {
        return Err(StoreError::Corruption("derived index entry count disagrees with snapshot".into()));
    }
    // A 513-byte all-FF bound sorts after every valid key (max 512 bytes).
    // Reconcile the complete linked-leaf key sequence, not only individual
    // point searches; this detects extra, absent and reordered leaf entries.
    let all = index.range(&[], &[0xff; MAX_KEY + 1])?;
    if all != entries {
        return Err(StoreError::Corruption("derived index leaf sequence differs from snapshot".into()));
    }
    for (key, pointer) in &entries {
        if index.get(key)? != Some(*pointer) {
            return Err(StoreError::Corruption("derived index pointer disagrees with snapshot".into()));
        }
    }
    Ok(IndexRecovery {
        checkpoint_lsn: lsn, rows: entries.len() as u64,
        data_pages: pages, rebuilt: false,
    })
}

/// Rebuild an *absent* index, using only the published snapshot as authority.
/// A corrupt existing index must be addressed by an explicit repair procedure;
/// silently overwriting it would conceal an integrity failure.
pub(crate) fn rebuild_missing(root: &Path, lsn: u64) -> StoreResult<IndexRecovery> {
    if index_path(root, lsn).exists() {
        return verify(root, lsn);
    }
    let (entries, _rows, pages) = scan_snapshot(root, lsn)?;
    let tmp = root.join(format!("index-{lsn}.rebuild.tmp"));
    if tmp.exists() {
        fs::remove_file(&tmp)?;
    }
    DiskBTree::build(&tmp, lsn, &entries)?;
    // Validate the new file before publication.
    {
        let mut generated = DiskBTree::open(&tmp, lsn)?;
        if generated.len() != entries.len() as u64 {
            return Err(StoreError::Corruption("rebuilt index count mismatch".into()));
        }
        if generated.range(&[], &[0xff; MAX_KEY + 1])? != entries {
            return Err(StoreError::Corruption("rebuilt index leaves do not match snapshot".into()));
        }
        for (key, pointer) in &entries {
            if generated.get(key)? != Some(*pointer) {
                return Err(StoreError::Corruption("rebuilt index pointer mismatch".into()));
            }
        }
    }
    if index_path(root, lsn).exists() {
        return Err(StoreError::Corruption("another index appeared during rebuild".into()));
    }
    fs::rename(&tmp, index_path(root, lsn))?;
    sync_directory(root)?;
    Ok(IndexRecovery {
        checkpoint_lsn: lsn, rows: entries.len() as u64,
        data_pages: pages, rebuilt: true,
    })
}

/// The filesystem directory must be synced after publishing index names on
/// Unix. Other platform durability requirements remain a separate gate.
#[cfg(unix)]
fn sync_directory(root: &Path) -> std::io::Result<()> {
    File::open(root)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_root: &Path) -> std::io::Result<()> {
    Ok(())
}
