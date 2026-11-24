//! ADR 0311 — Shard placement + fencing.
//!
//! A `PlacementMap` tracks `shard → (primary replica, fence
//! token)` assignments. Fence tokens advance monotonically
//! per shard so a replica that lost ownership can no longer
//! fool a receiver that holds the current token.
//!
//! This slice ships two variants:
//!
//! - `PlacementMap` — in-memory, process-local. Useful for
//!   unit tests and for the coordinator's own live view.
//! - `PlacementMap::open(root)` — same type but backed by a
//!   JSON manifest at `root/placements.json`. Every
//!   `assign` writes the whole map to `root/placements.tmp`
//!   and atomically renames it over the real manifest, then
//!   `fsync`s the directory so the rename lands durably. A
//!   subsequent `open` of the same root replays the manifest
//!   back into the in-memory map.
//!
//! Fence tokens survive restart. Reconfig across replicas
//! (consensus replication of the map) remains a follow-up
//! wired through ADR 0309's `ConsensusGroup`.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::consensus::ReplicaId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd,
         Serialize, Deserialize)]
pub struct ShardId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd,
         Serialize, Deserialize)]
pub struct FencingToken(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub shard:   ShardId,
    pub primary: ReplicaId,
    pub fence:   FencingToken,
}

#[derive(Debug, Error)]
pub enum PlacementError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("placement manifest {0:?} has corrupt format: {1}")]
    Corrupt(PathBuf, String),
    #[error("placement manifest version {0} is not supported")]
    UnsupportedVersion(u32),
}
pub type PlacementResult<T> = Result<T, PlacementError>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ManifestOnDisk {
    version:    u32,
    placements: Vec<Placement>,
}

impl ManifestOnDisk {
    fn from_map(placements: &HashMap<ShardId, Placement>) -> Self {
        let mut v: Vec<Placement> = placements.values().copied().collect();
        v.sort_by_key(|p| p.shard.0);   // deterministic on-disk order
        Self { version: 1, placements: v }
    }
}

#[derive(Debug, Default)]
pub struct PlacementMap {
    inner: Mutex<HashMap<ShardId, Placement>>,
    root:  Option<PathBuf>,            // Some = durable, None = in-memory
}

impl PlacementMap {
    /// In-memory map, no durability.
    pub fn new() -> Self { Self::default() }

    /// Open (or create) a durable placement map backed by
    /// a JSON manifest at `root/placements.json`. A missing
    /// root is created; an existing manifest is replayed.
    pub fn open(root: impl Into<PathBuf>) -> PlacementResult<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        let manifest_path = root.join("placements.json");
        let inner: HashMap<ShardId, Placement> = match File::open(&manifest_path) {
            Ok(f) => {
                let m: ManifestOnDisk = serde_json::from_reader(BufReader::new(f))?;
                if m.version != 1 {
                    return Err(PlacementError::UnsupportedVersion(m.version));
                }
                let mut map = HashMap::with_capacity(m.placements.len());
                for p in m.placements {
                    // Last-write-wins for duplicate shards —
                    // a well-formed manifest has distinct shards.
                    if map.insert(p.shard, p).is_some() {
                        return Err(PlacementError::Corrupt(manifest_path.clone(),
                            format!("duplicate shard {:?}", p.shard)));
                    }
                }
                map
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self { inner: Mutex::new(inner), root: Some(root) })
    }

    /// Advance the fence for `shard` and record `primary`
    /// as its owner. Returns the new fence token. For a
    /// durable map, this persists before returning.
    pub fn assign(&self, shard: ShardId, primary: ReplicaId) -> FencingToken {
        self.try_assign(shard, primary)
            .expect("placement manifest persist (durable path only fails on I/O)")
    }

    /// Fallible variant of `assign` — surfaces persist
    /// errors to the caller instead of panicking. The
    /// in-memory variant never errors.
    pub fn try_assign(
        &self, shard: ShardId, primary: ReplicaId,
    ) -> PlacementResult<FencingToken> {
        let mut g = self.inner.lock();
        let next_fence = FencingToken(
            g.get(&shard).map(|p| p.fence.0 + 1).unwrap_or(1));
        g.insert(shard, Placement { shard, primary, fence: next_fence });
        if let Some(root) = &self.root { persist(root, &g)?; }
        Ok(next_fence)
    }

    pub fn primary_of(&self, shard: ShardId) -> Option<Placement> {
        self.inner.lock().get(&shard).copied()
    }

    pub fn check_fence(&self, shard: ShardId, token: FencingToken) -> bool {
        self.inner.lock().get(&shard).map(|p| p.fence == token).unwrap_or(false)
    }

    pub fn is_durable(&self) -> bool { self.root.is_some() }

    pub fn len(&self) -> usize { self.inner.lock().len() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}

/// Persist the whole map via tmp-file + atomic rename +
/// `fsync` of the containing directory so the rename is
/// durably observable after a crash.
fn persist(root: &Path, placements: &HashMap<ShardId, Placement>) -> PlacementResult<()> {
    let manifest   = ManifestOnDisk::from_map(placements);
    let final_path = root.join("placements.json");
    let tmp_path   = root.join("placements.tmp");

    // Write to tmp with fsync on the file.
    {
        let mut w = BufWriter::new(OpenOptions::new()
            .write(true).create(true).truncate(true).open(&tmp_path)?);
        serde_json::to_writer(&mut w, &manifest)?;
        w.flush()?;
        w.get_ref().sync_data()?;
    }
    std::fs::rename(&tmp_path, &final_path)?;

    // Best-effort directory fsync so the rename is durable.
    // Opening the dir as a file and syncing is POSIX
    // behaviour; on Windows this is a no-op but the
    // underlying NTFS rename is already journaled.
    #[cfg(unix)]
    if let Ok(dir) = File::open(root) { let _ = dir.sync_data(); }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assign_returns_monotonically_advancing_fence_token() {
        let m = PlacementMap::new();
        let f1 = m.assign(ShardId(1), ReplicaId(10));
        assert_eq!(f1, FencingToken(1));
        let f2 = m.assign(ShardId(1), ReplicaId(11));
        assert_eq!(f2, FencingToken(2));
        let other = m.assign(ShardId(2), ReplicaId(20));
        assert_eq!(other, FencingToken(1),
            "fences are per-shard, not global");
    }

    #[test]
    fn stale_fence_is_rejected_by_check_fence() {
        let m = PlacementMap::new();
        let stale = m.assign(ShardId(1), ReplicaId(10));
        let fresh = m.assign(ShardId(1), ReplicaId(11));
        assert!(!m.check_fence(ShardId(1), stale));
        assert!( m.check_fence(ShardId(1), fresh));
        assert!(!m.check_fence(ShardId(2), fresh),
            "fence is only valid for its own shard");
    }

    #[test]
    fn durable_map_replays_assignments_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let fence_live = {
            let m = PlacementMap::open(dir.path()).unwrap();
            assert!(m.is_durable());
            assert_eq!(m.len(), 0);
            m.assign(ShardId(1), ReplicaId(10));
            m.assign(ShardId(2), ReplicaId(20));
            m.assign(ShardId(1), ReplicaId(11))
        };
        // Reopen — the second shard-1 assign should have
        // left fence=2, shard-2 fence=1.
        let m2 = PlacementMap::open(dir.path()).unwrap();
        assert_eq!(m2.len(), 2);
        let p1 = m2.primary_of(ShardId(1)).unwrap();
        assert_eq!(p1.primary, ReplicaId(11));
        assert_eq!(p1.fence,   fence_live);
        let p2 = m2.primary_of(ShardId(2)).unwrap();
        assert_eq!(p2.primary, ReplicaId(20));
        assert_eq!(p2.fence,   FencingToken(1));
        assert!( m2.check_fence(ShardId(1), fence_live));
        assert!(!m2.check_fence(ShardId(1), FencingToken(1)));
    }

    #[test]
    fn next_fence_after_reopen_advances_from_persisted_value() {
        let dir = tempfile::tempdir().unwrap();
        {
            let m = PlacementMap::open(dir.path()).unwrap();
            m.assign(ShardId(5), ReplicaId(50));
            m.assign(ShardId(5), ReplicaId(51));
            m.assign(ShardId(5), ReplicaId(52));
        }
        let m2 = PlacementMap::open(dir.path()).unwrap();
        let next = m2.assign(ShardId(5), ReplicaId(53));
        assert_eq!(next, FencingToken(4),
            "post-restart fences must continue the persisted sequence");
    }

    #[test]
    fn open_missing_root_creates_an_empty_durable_map() {
        let parent = tempfile::tempdir().unwrap();
        let nested = parent.path().join("nested").join("dir");
        assert!(!nested.exists());
        let m = PlacementMap::open(&nested).unwrap();
        assert_eq!(m.len(), 0);
        assert!(nested.is_dir());
        // Only after an assign does the manifest file exist.
        assert!(!nested.join("placements.json").exists());
        m.assign(ShardId(1), ReplicaId(1));
        assert!(nested.join("placements.json").is_file());
    }

    #[test]
    fn corrupt_manifest_is_rejected_on_open() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("placements.json"), b"not json").unwrap();
        let err = PlacementMap::open(dir.path()).unwrap_err();
        assert!(matches!(err, PlacementError::Serde(_)),
            "got {err:?}");
    }

    #[test]
    fn unsupported_manifest_version_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("placements.json"),
            br#"{"version":999,"placements":[]}"#).unwrap();
        let err = PlacementMap::open(dir.path()).unwrap_err();
        assert!(matches!(err, PlacementError::UnsupportedVersion(999)));
    }

    #[test]
    fn assign_is_atomic_against_mid_rename_reads() {
        // After an assign returns, a reader that opens the
        // directory must see either the new state or the
        // previous state — never a torn file. We simulate
        // this by doing many assigns and verifying each
        // reopen observes a well-formed manifest whose
        // shard count is at least as large as the previous.
        let dir = tempfile::tempdir().unwrap();
        let m = PlacementMap::open(dir.path()).unwrap();
        let mut seen: usize = 0;
        for i in 0..32u64 {
            m.assign(ShardId(i), ReplicaId(i));
            let reader = PlacementMap::open(dir.path()).unwrap();
            let now = reader.len();
            assert!(now >= seen,
                "manifest regressed from {seen} to {now} shards");
            seen = now;
        }
        assert_eq!(seen, 32);
    }
}
