//! ADR 0168 — projection segment catalog.
//!
//! Composes with ADR 0163 segment writer + ADR 0164
//! scheduler to give callers a discoverable index of
//! projection segments without touching the main
//! relational Catalog. A separate JSON index file lives
//! alongside the `.brps` segments.
//!
//! On-disk layout (one directory per table):
//!
//! ```text
//! <table_dir>/
//!   p-00000000000000000010.brps
//!   p-00000000000000000020.brps
//!   index.json                  <- ADR 0168
//! ```
//!
//! `index.json` is a serde-serialized `ProjectionIndex`
//! with one entry per committed segment.
//!
//! Honest scope:
//! - Append-only. `register_segment` reads the current
//!   index, appends, writes a temp file, renames atomically.
//! - No fsync of parent directory (caller does that if
//!   they need an fsync barrier).
//! - Readers call `load_index(dir)`; stale indexes are
//!   rejected via CRC-free JSON (the segment file itself
//!   carries CRC per ADR 0163).
//!
//! Out of scope:
//! - Garbage collection of older segments.
//! - Hierarchical (partition) layout.
//! - Catalog integration — this catalog is deliberately
//!   decoupled from the main Catalog so OLAP state
//!   doesn't bloat OLTP metadata.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentEntry {
    pub as_of_lsn: u64,
    pub filename: String,
    pub bytes: u64,
    /// Micros since UNIX epoch at registration time.
    pub registered_at_micros: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectionIndex {
    pub segments: Vec<SegmentEntry>,
}

impl ProjectionIndex {
    pub fn new() -> Self { Self::default() }

    pub fn latest_lsn(&self) -> Option<u64> {
        self.segments.iter().map(|s| s.as_of_lsn).max()
    }

    pub fn len(&self) -> usize { self.segments.len() }
    pub fn is_empty(&self) -> bool { self.segments.is_empty() }

    /// Return the segment covering `target_lsn` (latest
    /// as_of_lsn ≤ target), if any.
    pub fn segment_covering(&self, target_lsn: u64) -> Option<&SegmentEntry> {
        self.segments.iter()
            .filter(|s| s.as_of_lsn <= target_lsn)
            .max_by_key(|s| s.as_of_lsn)
    }
}

#[derive(Debug, Error)]
pub enum ProjectionCatalogError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("duplicate as_of_lsn {lsn} in index")]
    DuplicateLsn { lsn: u64 },
}

pub type CatalogResult<T> = std::result::Result<T, ProjectionCatalogError>;

const INDEX_FILENAME: &str = "index.json";

/// ADR 0168 — register a new segment in the index for
/// the given directory. Reads current index, appends,
/// writes atomically via rename.
pub fn register_segment(
    dir: &Path,
    as_of_lsn: u64,
    filename: &str,
    bytes: u64,
) -> CatalogResult<()> {
    let mut index = load_index(dir)?;
    if index.segments.iter().any(|s| s.as_of_lsn == as_of_lsn) {
        return Err(ProjectionCatalogError::DuplicateLsn { lsn: as_of_lsn });
    }
    index.segments.push(SegmentEntry {
        as_of_lsn,
        filename: filename.into(),
        bytes,
        registered_at_micros: now_micros(),
    });
    // Keep LSN-sorted for readers that rely on order.
    index.segments.sort_by_key(|s| s.as_of_lsn);
    write_index_atomic(dir, &index)?;
    Ok(())
}

/// ADR 0168 — load the current index. Returns an empty
/// index if the file doesn't exist.
pub fn load_index(dir: &Path) -> CatalogResult<ProjectionIndex> {
    let path = dir.join(INDEX_FILENAME);
    if !path.exists() {
        return Ok(ProjectionIndex::new());
    }
    let mut f = File::open(&path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(serde_json::from_slice(&buf)?)
}

fn write_index_atomic(dir: &Path, index: &ProjectionIndex) -> CatalogResult<()> {
    fs::create_dir_all(dir)?;
    let final_path = dir.join(INDEX_FILENAME);
    let tmp_path = dir.join(format!("{INDEX_FILENAME}.tmp"));
    let bytes = serde_json::to_vec(index)?;
    {
        let mut f = File::create(&tmp_path)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp_path, &final_path)?;
    Ok(())
}

fn now_micros() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn empty_directory_loads_as_empty_index() {
        let dir = tempdir().unwrap();
        let index = load_index(dir.path()).unwrap();
        assert!(index.is_empty());
    }

    #[test]
    fn register_then_load_round_trips_entries() {
        let dir = tempdir().unwrap();
        register_segment(dir.path(), 10, "p-00...0010.brps", 1234).unwrap();
        register_segment(dir.path(), 20, "p-00...0020.brps", 5678).unwrap();
        let index = load_index(dir.path()).unwrap();
        assert_eq!(index.len(), 2);
        assert_eq!(index.latest_lsn(), Some(20));
    }

    #[test]
    fn duplicate_lsn_rejected() {
        let dir = tempdir().unwrap();
        register_segment(dir.path(), 10, "a.brps", 1).unwrap();
        let err = register_segment(dir.path(), 10, "b.brps", 1).unwrap_err();
        assert!(matches!(err, ProjectionCatalogError::DuplicateLsn { lsn: 10 }));
    }

    #[test]
    fn segment_covering_picks_latest_at_or_below_target() {
        let dir = tempdir().unwrap();
        register_segment(dir.path(), 10, "a.brps", 1).unwrap();
        register_segment(dir.path(), 20, "b.brps", 1).unwrap();
        register_segment(dir.path(), 30, "c.brps", 1).unwrap();
        let index = load_index(dir.path()).unwrap();
        assert_eq!(index.segment_covering(25).unwrap().as_of_lsn, 20);
        assert_eq!(index.segment_covering(10).unwrap().as_of_lsn, 10);
        assert!(index.segment_covering(5).is_none());
        assert_eq!(index.segment_covering(1000).unwrap().as_of_lsn, 30);
    }

    #[test]
    fn index_is_sorted_by_lsn_after_out_of_order_inserts() {
        let dir = tempdir().unwrap();
        register_segment(dir.path(), 30, "c.brps", 1).unwrap();
        register_segment(dir.path(), 10, "a.brps", 1).unwrap();
        register_segment(dir.path(), 20, "b.brps", 1).unwrap();
        let index = load_index(dir.path()).unwrap();
        let lsns: Vec<_> = index.segments.iter().map(|s| s.as_of_lsn).collect();
        assert_eq!(lsns, vec![10, 20, 30]);
    }

    #[test]
    fn index_json_is_atomic_via_rename() {
        let dir = tempdir().unwrap();
        register_segment(dir.path(), 1, "x.brps", 100).unwrap();
        // No .tmp file left behind.
        let tmp = dir.path().join(format!("{INDEX_FILENAME}.tmp"));
        assert!(!tmp.exists());
    }

    #[test]
    fn entries_carry_registration_timestamp() {
        let dir = tempdir().unwrap();
        register_segment(dir.path(), 1, "a.brps", 0).unwrap();
        let index = load_index(dir.path()).unwrap();
        assert!(index.segments[0].registered_at_micros > 0);
    }
}
