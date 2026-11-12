//! G20: native, WAL-backed, single-store MVCC foundation.
//!
//! This is an opt-in row-level API, deliberately NOT yet wired into the G16
//! multi-table SQL executor. It keeps historical versions in memory for active
//! snapshots, but persists only current committed values through G3's atomic
//! BRWL batch protocol. On restart, the latest durable state is recovered;
//! historical versions from prior processes are not recoverable.
//!
//! Writers touching disjoint keys can commit from concurrent snapshots. A
//! modified key that changed after a transaction's start LSN conflicts,
//! including changes that were subsequently undone (ABA protection).
//! Transactions that perform full scans use conservative whole-store LSN
//! validation to protect against untracked range/predicate phantoms.
//!
//! This implements a bounded SNAPSHOT-ISOLATION FOUNDATION, not serializable
//! SQL, persistent time travel, catalog MVCC, or cluster replication. The
//! in-memory history's retention grows with long-lived readers.

use std::collections::BTreeMap;
use std::path::Path;

use parking_lot::Mutex;
use thiserror::Error;

use crate::row_store::{RowMutation, RowStore, StoreError};

#[derive(Debug, Error)]
pub enum MvccError {
    #[error(transparent)]
    Storage(#[from] StoreError),
    #[error("MVCC write conflict on key; a newer committed version exists")]
    WriteConflict,
    #[error("MVCC range/predicate conflict after a full-table scan")]
    ScanConflict,
    #[error("MVCC row already exists at transaction snapshot")]
    DuplicateKey,
    #[error("MVCC row does not exist at transaction snapshot")]
    NotFound,
    #[error("MVCC sequence reached maximum representable value")]
    SequenceExhausted,
    #[error("MVCC state is inconsistent with the underlying committed WAL: {0}")]
    Corruption(String),
}

pub type MvccResult<T> = Result<T, MvccError>;

#[derive(Clone, Debug)]
struct Version {
    lsn: u64,
    // None means a committed deletion tombstone.
    value: Option<Vec<u8>>,
}

#[derive(Default)]
struct History {
    // Exactly one version per changed key per committed WAL batch.
    versions: BTreeMap<Vec<u8>, Vec<Version>>,
    // start LSN -> number of active transactions pinned at that position.
    readers: BTreeMap<u64, usize>,
    committed_lsn: u64,
}

impl History {
    fn visible(&self, key: &[u8], at: u64) -> Option<Vec<u8>> {
        self.versions.get(key)
            .and_then(|chain| chain.iter().rev().find(|v| v.lsn <= at))
            .and_then(|v| v.value.clone())
    }

    fn prune(&mut self) {
        // Keep the last version <= the oldest active start LSN, and all
        // versions after it. If no readers exist, keep only the latest.
        let oldest = self.readers.keys().next().copied();
        self.versions.retain(|_, chain| {
            if chain.len() < 2 { return !chain.is_empty(); }
            let retain_from = match oldest {
                Some(oldest_lsn) => {
                    let first_after = chain.partition_point(|v| v.lsn <= oldest_lsn);
                    first_after.saturating_sub(1)
                }
                None => chain.len() - 1,
            };
            if retain_from > 0 { chain.drain(..retain_from); }
            // A lone unpinned tombstone can be dropped. Future snapshots
            // still correctly see absence; new inserts are not rejected.
            !(oldest.is_none() && chain.len() == 1 && chain[0].value.is_none())
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MvccStats {
    pub committed_lsn: u64,
    pub active_snapshots: usize,
    pub pinned_positions: usize,
    pub keys_retained: usize,
    pub versions_retained: usize,
}

/// Exclusive single-process owner of one WAL and one version history.
/// Do not separately open or mutate its RowStore path while it is live.
pub struct MvccStore {
    storage: RowStore,
    history: Mutex<History>,
}

impl MvccStore {
    pub fn open(root: impl AsRef<Path>) -> MvccResult<Self> {
        let storage = RowStore::open(root)?;
        let (committed_lsn, rows) = storage.snapshot_with_lsn();
        let mut history = History {
            committed_lsn,
            ..History::default()
        };
        // Seed only the latest durable values. Earlier versions, if any,
        // were not retained on disk and must never be offered after restart.
        for (key, value) in rows {
            history.versions.insert(key, vec![Version {
                lsn: committed_lsn,
                value: Some(value),
            }]);
        }
        Ok(Self { storage, history: Mutex::new(history) })
    }

    pub fn begin(&self) -> MvccTransaction<'_> {
        let mut state = self.history.lock();
        let start_lsn = state.committed_lsn;
        *state.readers.entry(start_lsn).or_default() += 1;
        MvccTransaction {
            store: self,
            start_lsn,
            pending: BTreeMap::new(),
            did_scan: false,
            finished: false,
        }
    }

    pub fn committed_lsn(&self) -> u64 {
        self.history.lock().committed_lsn
    }

    pub fn stats(&self) -> MvccStats {
        let state = self.history.lock();
        MvccStats {
            committed_lsn: state.committed_lsn,
            active_snapshots: state.readers.values().sum(),
            pinned_positions: state.readers.len(),
            keys_retained: state.versions.len(),
            versions_retained: state.versions.values().map(Vec::len).sum(),
        }
    }

    pub fn checkpoint(&self) -> MvccResult<u64> {
        // Checkpointing uses the same single RowStore and retains only the
        // latest durable row image; pinned read versions remain in RAM.
        let state = self.history.lock();
        if state.committed_lsn != self.storage.last_lsn() {
            return Err(MvccError::Corruption("checkpoint LSN disagreement".into()));
        }
        Ok(self.storage.checkpoint()?)
    }

    fn unpin(&self, lsn: u64) {
        let mut state = self.history.lock();
        if let Some(count) = state.readers.get_mut(&lsn) {
            *count -= 1;
            if *count == 0 { state.readers.remove(&lsn); }
        }
        state.prune();
    }
}

/// Owned staged mutations and a pinned visibility horizon. Dropping this
/// transaction without commit is a rollback: no WAL record is appended.
pub struct MvccTransaction<'a> {
    store: &'a MvccStore,
    start_lsn: u64,
    pending: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    did_scan: bool,
    finished: bool,
}

impl MvccTransaction<'_> {
    pub fn start_lsn(&self) -> u64 { self.start_lsn }

    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        if let Some(value) = self.pending.get(key) { return value.clone(); }
        self.store.history.lock().visible(key, self.start_lsn)
    }

    pub fn scan(&mut self) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.did_scan = true;
        let state = self.store.history.lock();
        let mut result = state.versions.keys().filter_map(|key| state.visible(key, self.start_lsn)
                .map(|value| (key.clone(), value)))
            .collect::<BTreeMap<_,_>>();
        for (key, value) in &self.pending {
            match value {
                Some(value) => { result.insert(key.clone(), value.clone()); }
                None => { result.remove(key); }
            }
        }
        result.into_iter().collect()
    }

    /// Stage a new key. Physical key/value constraints are checked by
    /// RowStore before any durable WAL append at commit.
    pub fn insert(&mut self, key: &[u8], value: &[u8]) -> MvccResult<()> {
        if self.get(key).is_some() { return Err(MvccError::DuplicateKey); }
        self.pending.insert(key.to_vec(), Some(value.to_vec()));
        Ok(())
    }

    pub fn replace(&mut self, key: &[u8], value: &[u8]) -> MvccResult<()> {
        if self.get(key).is_none() { return Err(MvccError::NotFound); }
        self.pending.insert(key.to_vec(), Some(value.to_vec()));
        Ok(())
    }

    pub fn delete(&mut self, key: &[u8]) -> MvccResult<()> {
        if self.get(key).is_none() { return Err(MvccError::NotFound); }
        self.pending.insert(key.to_vec(), None);
        Ok(())
    }

    pub fn rollback(self) {
        // Drop unpins the snapshot; no WAL write takes place.
    }

    pub fn commit(mut self) -> MvccResult<u64> {
        if self.pending.is_empty() {
            return Ok(self.start_lsn);
        }
        let mut state = self.store.history.lock();
        if self.did_scan && state.committed_lsn != self.start_lsn {
            return Err(MvccError::ScanConflict);
        }
        for key in self.pending.keys() {
            if state.versions.get(key)
                .and_then(|chain| chain.last())
                .is_some_and(|last| last.lsn > self.start_lsn)
            {
                return Err(MvccError::WriteConflict);
            }
        }
        // RowStore's actual sequence and our committed history MUST agree.
        // Only this facade is permitted to change this underlying RowStore.
        if self.store.storage.last_lsn() != state.committed_lsn {
            return Err(MvccError::Corruption("WAL sequence changed outside MVCC".into()));
        }
        let expected = state.committed_lsn;
        let operations = self.pending.iter().map(|(key, value)| {
            match value {
                Some(value) => RowMutation::Put {
                    key: key.clone(), value: value.clone(),
                },
                None => RowMutation::Delete { key: key.clone() },
            }
        }).collect::<Vec<_>>();
        let lsn = match self.store.storage.commit_batch(Some(expected), &operations) {
            Ok(lsn) => lsn,
            Err(StoreError::Conflict { .. }) => return Err(
                MvccError::Corruption("unexpected out-of-band WAL writer".into())
            ),
            Err(error) => return Err(error.into()),
        };
        if lsn != expected.checked_add(1).ok_or(MvccError::SequenceExhausted)? {
            return Err(MvccError::Corruption("nonconsecutive WAL commit LSN".into()));
        }
        for (key, value) in std::mem::take(&mut self.pending) {
            state.versions.entry(key).or_default().push(Version { lsn, value });
        }
        state.committed_lsn = lsn;
        // Release this snapshot pin while already holding the history mutex.
        if let Some(count) = state.readers.get_mut(&self.start_lsn) {
            *count -= 1;
            if *count == 0 { state.readers.remove(&self.start_lsn); }
        }
        self.finished = true;
        state.prune();
        Ok(lsn)
    }
}

impl Drop for MvccTransaction<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.store.unpin(self.start_lsn);
            self.finished = true;
        }
    }
}
