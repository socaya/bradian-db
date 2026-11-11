//! Snapshot handles for concurrent reads.
//!
//! A `Snapshot` is a cheap, reference-counted handle naming a (generation, delta_version)
//! pair. Compaction cannot delete segments or delta entries referenced by live snapshots.

use std::sync::Arc;

use parking_lot::Mutex;

/// Monotonic counter advanced when the in-memory delta is flushed to a new segment.
pub type DeltaVersion = u64;

#[derive(Debug, Clone)]
pub struct Snapshot {
    inner: Arc<SnapshotInner>,
}

#[derive(Debug)]
struct SnapshotInner {
    generation: u64,
    delta_version: DeltaVersion,
    retainer: Arc<Mutex<SnapshotRefs>>,
}

/// Internal reference map held by `Partition`. Tracks how many live snapshots reference each
/// generation so compaction knows when it is safe to GC superseded segments.
#[derive(Debug, Default)]
pub struct SnapshotRefs {
    counts: ahash::AHashMap<u64, usize>,
}

impl SnapshotRefs {
    pub fn acquire(&mut self, generation: u64) {
        *self.counts.entry(generation).or_default() += 1;
    }
    pub fn release(&mut self, generation: u64) {
        if let Some(c) = self.counts.get_mut(&generation) {
            *c = c.saturating_sub(1);
            if *c == 0 {
                self.counts.remove(&generation);
            }
        }
    }
    pub fn is_referenced(&self, generation: u64) -> bool {
        self.counts.get(&generation).copied().unwrap_or(0) > 0
    }
}

pub type SnapshotRef = Arc<Mutex<SnapshotRefs>>;

impl Snapshot {
    pub fn new(generation: u64, delta_version: DeltaVersion, refs: SnapshotRef) -> Self {
        refs.lock().acquire(generation);
        Self {
            inner: Arc::new(SnapshotInner {
                generation,
                delta_version,
                retainer: refs,
            }),
        }
    }

    pub fn generation(&self) -> u64 {
        self.inner.generation
    }
    pub fn delta_version(&self) -> DeltaVersion {
        self.inner.delta_version
    }
}

impl Drop for SnapshotInner {
    fn drop(&mut self) {
        self.retainer.lock().release(self.generation);
    }
}
