//! G22 durable MVCC history over one authoritative G3 RowStore WAL.
//!
//! This is an explicitly versioned, opt-in storage format. Every committed
//! user-key change is stored as an immutable (key, commit_lsn) record inside
//! the SAME synchronized BRWL batch as all other changes in its transaction.
//! There is no second WAL, no two-phase commit, and no separate history file.
//! Historical point reads survive process restart and G9 checkpoints.
//!
//! G22 does NOT silently migrate G20/G21 stores, provide persistent sessions,
//! implement MVCC SQL or guarantee serializable isolation. Manual retention
//! advancement can delete old historical versions only after a durable floor
//! marker is synced; readers below the floor are explicitly rejected.

use std::collections::BTreeMap;
use std::path::Path;

use parking_lot::Mutex;
use thiserror::Error;

use crate::row_store::{RowMutation, RowStore, StoreError};
use crate::backup::BackupReceipt;

const FORMAT_KEY: &[u8] = b"\0bradian.durable-mvcc.format";
const FLOOR_KEY: &[u8] = b"\0bradian.durable-mvcc.floor";
const FORMAT: &[u8] = b"BRADIAN-DURABLE-MVCC-V1";
const VKEY: &[u8; 4] = b"DVH1";
const MAX_USER_KEY: usize = 498; // 4 prefix + 2 length + 8 LSN <= RowStore max 512
const MAX_USER_VALUE: usize = 3545; // 1 version flag + inline payload <= G3 row limit
const MAX_WRITE_KEYS: usize = 256;
const GC_BATCH: usize = 256;

#[derive(Debug, Error)]
pub enum DurableMvccError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("durable MVCC corruption or incompatible format: {0}")]
    Corruption(String),
    #[error("durable MVCC key or value is outside the supported inline capacity")]
    Capacity,
    #[error("durable MVCC key is already present in the transaction")]
    DuplicateKey,
    #[error("durable MVCC key is absent in the transaction")]
    NotFound,
    #[error("newer committed version conflicts with the transaction's snapshot")]
    Conflict,
    #[error("a full scan observed an outdated global sequence")]
    ScanConflict,
    #[error("snapshot LSN {requested} is outside the retained range {floor}..={latest}")]
    SnapshotUnavailable { requested: u64, floor: u64, latest: u64 },
    #[error("cannot advance retention past an active pinned snapshot")]
    PinnedSnapshot,
    #[error("transaction or WAL sequence capacity exhausted")]
    CapacityExhausted,
}

pub type DurableMvccResult<T> = Result<T, DurableMvccError>;

#[derive(Clone)]
struct HistoricalVersion {
    lsn: u64,
    value: Option<Vec<u8>>,
}

struct State {
    latest: u64,
    floor: u64,
    versions: BTreeMap<Vec<u8>, Vec<HistoricalVersion>>,
    pins: BTreeMap<u64, usize>,
}

pub struct DurableMvccStore {
    rows: RowStore,
    state: Mutex<State>,
}

fn history_key(key: &[u8], lsn: u64) -> Vec<u8> {
    let mut result = Vec::with_capacity(14 + key.len());
    result.extend_from_slice(VKEY);
    result.extend_from_slice(&(key.len() as u16).to_be_bytes());
    result.extend_from_slice(key);
    result.extend_from_slice(&lsn.to_be_bytes());
    result
}

fn parse_history_key(raw: &[u8]) -> DurableMvccResult<(Vec<u8>, u64)> {
    if raw.len() < 15 || !raw.starts_with(VKEY) {
        return Err(DurableMvccError::Corruption("invalid history key".into()));
    }
    let size = u16::from_be_bytes(raw[4..6].try_into().unwrap()) as usize;
    if size == 0 || size > MAX_USER_KEY || raw.len() != size + 14 {
        return Err(DurableMvccError::Corruption("invalid historical user-key length".into()));
    }
    let key = raw[6..6 + size].to_vec();
    let lsn = u64::from_be_bytes(raw[6 + size..].try_into().unwrap());
    Ok((key, lsn))
}

fn version_value(value: &Option<Vec<u8>>) -> Vec<u8> {
    match value {
        Some(bytes) => {
            let mut encoded = Vec::with_capacity(1 + bytes.len());
            encoded.push(1);
            encoded.extend_from_slice(bytes);
            encoded
        }
        None => vec![0],
    }
}

fn decode_version(raw: &[u8]) -> DurableMvccResult<Option<Vec<u8>>> {
    match raw {
        [0] => Ok(None),
        [1, payload @ ..] if payload.len() <= MAX_USER_VALUE => Ok(Some(payload.to_vec())),
        _ => Err(DurableMvccError::Corruption("invalid versioned value tag/size".into())),
    }
}

fn visible(versions: &[HistoricalVersion], horizon: u64) -> Option<Vec<u8>> {
    versions.iter().rev().find(|v| v.lsn <= horizon).and_then(|v| v.value.clone())
}

fn validate_user(key: &[u8], val: Option<&[u8]>) -> DurableMvccResult<()> {
    if key.is_empty() || key.len() > MAX_USER_KEY ||
        val.is_some_and(|v| v.len() > MAX_USER_VALUE)
    {
        return Err(DurableMvccError::Capacity);
    }
    Ok(())
}

impl DurableMvccStore {
    /// New-format directories are initialized with one atomic G3 WAL batch.
    /// Nonempty directories without the matching marker fail closed.
    pub fn open(path: impl AsRef<Path>) -> DurableMvccResult<Self> {
        let rows = RowStore::open(path)?;
        if rows.is_empty() && rows.last_lsn() == 0 {
            rows.commit_batch(None, &[
                RowMutation::Put { key: FORMAT_KEY.to_vec(), value: FORMAT.to_vec() },
                RowMutation::Put { key: FLOOR_KEY.to_vec(), value: 0_u64.to_be_bytes().to_vec() },
            ])?;
        }
        if rows.get(FORMAT_KEY).as_deref() != Some(FORMAT) {
            return Err(DurableMvccError::Corruption("missing durable-history format marker".into()));
        }
        let (latest, entries) = rows.snapshot_with_lsn();
        let mut floor = None;
        let mut versions: BTreeMap<Vec<u8>, Vec<HistoricalVersion>> = BTreeMap::new();
        for (key, value) in entries {
            if key == FORMAT_KEY {
                if value != FORMAT {
                    return Err(DurableMvccError::Corruption("invalid format marker".into()));
                }
                continue;
            }
            if key == FLOOR_KEY {
                if value.len() != 8 {
                    return Err(DurableMvccError::Corruption("invalid retention floor".into()));
                }
                floor = Some(u64::from_be_bytes(value.try_into().unwrap()));
                continue;
            }
            let (user_key, lsn) = parse_history_key(&key)?;
            if lsn <= 1 || lsn > latest {
                return Err(DurableMvccError::Corruption("history version LSN out of range".into()));
            }
            versions.entry(user_key).or_default().push(HistoricalVersion {
                lsn, value: decode_version(&value)?,
            });
        }
        let floor = floor.ok_or_else(|| DurableMvccError::Corruption(
            "missing retention floor".into()
        ))?;
        if floor > latest {
            return Err(DurableMvccError::Corruption("retention floor exceeds WAL sequence".into()));
        }
        for chain in versions.values_mut() {
            chain.sort_unstable_by_key(|version| version.lsn);
            if chain.windows(2).any(|pair| pair[0].lsn == pair[1].lsn) {
                return Err(DurableMvccError::Corruption("duplicate version LSN".into()));
            }
        }
        Ok(Self {
            rows,
            state: Mutex::new(State { latest, floor, versions, pins: BTreeMap::new() }),
        })
    }

    /// G10 captures the exact version-key RowStore, including the retention
    /// floor, commit-tagged rows and tombstones. It does not transfer active
    /// in-process transaction pins into the restored process.
    pub fn backup_to(&self, destination: impl AsRef<Path>) -> DurableMvccResult<BackupReceipt> {
        let state = self.state.lock();
        if state.latest != self.rows.last_lsn() {
            return Err(DurableMvccError::Corruption("WAL changed outside durable MVCC".into()));
        }
        Ok(self.rows.backup_to(destination)?)
    }

    /// Restore only into an absent directory, with full G10 verification.
    /// Call open() on the restored directory to verify G22's version format.
    pub fn restore_backup(
        backup: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> DurableMvccResult<BackupReceipt> {
        Ok(RowStore::restore_backup(backup, destination)?)
    }

    /// Inspect all physically retained versions, including tombstones. Intended
    /// for offline typed-history verification before accepting SQL traffic;
    /// the caller must validate payloads and namespaces against catalog pins.
    /// This makes an owned snapshot, so it is not a streaming production audit.
    pub(crate) fn retained_versions(&self) -> Vec<(Vec<u8>, u64, Option<Vec<u8>>)> {
        let state = self.state.lock();
        state.versions.iter().flat_map(|(key, chain)| {
            chain.iter().map(|v| (key.clone(), v.lsn, v.value.clone()))
        }).collect()
    }

    pub fn latest_lsn(&self) -> u64 { self.state.lock().latest }
    pub fn retention_floor(&self) -> u64 { self.state.lock().floor }
    pub fn checkpoint(&self) -> DurableMvccResult<u64> {
        let state = self.state.lock();
        if state.latest != self.rows.last_lsn() {
            return Err(DurableMvccError::Corruption("RowStore LSN changed outside durable MVCC".into()));
        }
        Ok(self.rows.checkpoint()?)
    }

    /// A point-in-time historical read; unlike G20, earlier committed
    /// versions remain queryable after restart when retained.
    pub fn read_as_of(&self, key: &[u8], lsn: u64) -> DurableMvccResult<Option<Vec<u8>>> {
        validate_user(key, None)?;
        let state = self.state.lock();
        if lsn < state.floor || lsn > state.latest {
            return Err(DurableMvccError::SnapshotUnavailable {
                requested: lsn, floor: state.floor, latest: state.latest
            });
        }
        Ok(state.versions.get(key).and_then(|vs| visible(vs, lsn)))
    }

    /// A committed historical snapshot across all user keys at one WAL
    /// horizon. This is a read-only owned result, not a resumed transaction.
    /// Version history is reconstructed from native WAL/checkpoints on open.
    pub fn scan_as_of(&self, lsn: u64) -> DurableMvccResult<Vec<(Vec<u8>, Vec<u8>)>> {
        let state = self.state.lock();
        if lsn < state.floor || lsn > state.latest {
            return Err(DurableMvccError::SnapshotUnavailable {
                requested: lsn, floor: state.floor, latest: state.latest,
            });
        }
        Ok(state.versions.iter().filter_map(|(key, versions)| {
            visible(versions, lsn).map(|value| (key.clone(), value))
        }).collect())
    }

    pub fn begin(&self) -> DurableMvccTransaction<'_> {
        let mut state = self.state.lock();
        let start = state.latest;
        *state.pins.entry(start).or_default() += 1;
        DurableMvccTransaction {
            owner: self, start, pending: BTreeMap::new(), scanned: false, released: false,
        }
    }

    pub fn stats(&self) -> DurableMvccStats {
        let state = self.state.lock();
        DurableMvccStats {
            latest_lsn: state.latest,
            retention_floor: state.floor,
            active_snapshots: state.pins.values().sum(),
            persisted_versions: state.versions.values().map(Vec::len).sum(),
            known_keys: state.versions.len(),
        }
    }

    fn unpin(&self, lsn: u64) {
        let mut state = self.state.lock();
        if let Some(count) = state.pins.get_mut(&lsn) {
            *count -= 1;
            if *count == 0 { state.pins.remove(&lsn); }
        }
    }

    /// Advance the durable history floor, then physically remove obsolete
    /// version records in bounded G3 WAL batches. The floor is committed
    /// BEFORE deletion, so interrupted GC can be resumed safely on restart.
    /// The baseline newest version <= floor is always preserved per key.
    pub fn prune_through(&self, floor: u64) -> DurableMvccResult<usize> {
        let mut state = self.state.lock();
        if floor < state.floor || floor > state.latest {
            return Err(DurableMvccError::SnapshotUnavailable {
                requested: floor, floor: state.floor, latest: state.latest,
            });
        }
        if state.pins.keys().next().is_some_and(|oldest| *oldest < floor) {
            return Err(DurableMvccError::PinnedSnapshot);
        }
        if floor > state.floor {
            let lsn = self.rows.commit_batch(Some(state.latest), &[
                RowMutation::Put { key: FLOOR_KEY.to_vec(), value: floor.to_be_bytes().to_vec() },
            ])?;
            state.latest = lsn;
            state.floor = floor;
        }
        let mut doomed = Vec::new();
        for (key, chain) in &state.versions {
            let first_after = chain.partition_point(|v| v.lsn <= floor);
            let obsolete = first_after.saturating_sub(1);
            for version in chain.iter().take(obsolete) {
                doomed.push(history_key(key, version.lsn));
            }
        }
        let n = doomed.len();
        for chunk in doomed.chunks(GC_BATCH) {
            let deletes = chunk.iter().map(|key| RowMutation::Delete { key: key.clone() })
                .collect::<Vec<_>>();
            let lsn = self.rows.commit_batch(Some(state.latest), &deletes)?;
            state.latest = lsn;
            // Update only successfully deleted in-memory keys; interrupted
            // deletion can be retried on next open/prune through same floor.
            for key in chunk {
                let (user, old_lsn) = parse_history_key(key)?;
                if let Some(chain) = state.versions.get_mut(&user) {
                    chain.retain(|version| version.lsn != old_lsn);
                }
            }
        }
        Ok(n)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableMvccStats {
    pub latest_lsn: u64,
    pub retention_floor: u64,
    pub active_snapshots: usize,
    pub persisted_versions: usize,
    pub known_keys: usize,
}

pub struct DurableMvccTransaction<'a> {
    owner: &'a DurableMvccStore,
    start: u64,
    pending: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    scanned: bool,
    released: bool,
}

impl DurableMvccTransaction<'_> {
    pub fn start_lsn(&self) -> u64 { self.start }

    pub fn get(&self, key: &[u8]) -> DurableMvccResult<Option<Vec<u8>>> {
        validate_user(key, None)?;
        if let Some(value) = self.pending.get(key) { return Ok(value.clone()); }
        let state = self.owner.state.lock();
        Ok(state.versions.get(key).and_then(|chain| visible(chain, self.start)))
    }

    pub fn scan(&mut self) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.scanned = true;
        let state = self.owner.state.lock();
        let mut visible_rows = state.versions.iter().filter_map(|(key, chain)| {
            visible(chain, self.start).map(|value| (key.clone(), value))
        }).collect::<BTreeMap<_,_>>();
        for (key, value) in &self.pending {
            if let Some(value) = value {
                visible_rows.insert(key.clone(), value.clone());
            } else {
                visible_rows.remove(key);
            }
        }
        visible_rows.into_iter().collect()
    }

    pub fn insert(&mut self, key: &[u8], value: &[u8]) -> DurableMvccResult<()> {
        validate_user(key, Some(value))?;
        if self.get(key)?.is_some() { return Err(DurableMvccError::DuplicateKey); }
        self.pending.insert(key.to_vec(), Some(value.to_vec()));
        Ok(())
    }

    pub fn replace(&mut self, key: &[u8], value: &[u8]) -> DurableMvccResult<()> {
        validate_user(key, Some(value))?;
        if self.get(key)?.is_none() { return Err(DurableMvccError::NotFound); }
        self.pending.insert(key.to_vec(), Some(value.to_vec()));
        Ok(())
    }

    pub fn delete(&mut self, key: &[u8]) -> DurableMvccResult<()> {
        validate_user(key, None)?;
        if self.get(key)?.is_none() { return Err(DurableMvccError::NotFound); }
        self.pending.insert(key.to_vec(), None);
        Ok(())
    }

    pub fn rollback(self) {}

    pub fn commit(mut self) -> DurableMvccResult<u64> {
        if self.pending.is_empty() { return Ok(self.start); }
        if self.pending.len() > MAX_WRITE_KEYS { return Err(DurableMvccError::CapacityExhausted); }
        let mut state = self.owner.state.lock();
        if self.scanned && state.latest != self.start {
            return Err(DurableMvccError::ScanConflict);
        }
        for key in self.pending.keys() {
            if state.versions.get(key).and_then(|vs| vs.last())
                .is_some_and(|v| v.lsn > self.start)
            {
                return Err(DurableMvccError::Conflict);
            }
        }
        if state.latest != self.owner.rows.last_lsn() {
            return Err(DurableMvccError::Corruption("out-of-band storage write".into()));
        }
        let version_lsn = state.latest.checked_add(1)
            .ok_or(DurableMvccError::CapacityExhausted)?;
        let mutations = self.pending.iter().map(|(key, value)| RowMutation::Put {
            key: history_key(key, version_lsn),
            value: version_value(value),
        }).collect::<Vec<_>>();
        let actual = self.owner.rows.commit_batch(Some(state.latest), &mutations)?;
        if actual != version_lsn {
            return Err(DurableMvccError::Corruption("unexpected WAL commit sequence".into()));
        }
        for (key, value) in std::mem::take(&mut self.pending) {
            state.versions.entry(key).or_default().push(HistoricalVersion {
                lsn: actual, value,
            });
        }
        state.latest = actual;
        if let Some(count) = state.pins.get_mut(&self.start) {
            *count -= 1;
            if *count == 0 { state.pins.remove(&self.start); }
        }
        self.released = true;
        Ok(actual)
    }
}

impl Drop for DurableMvccTransaction<'_> {
    fn drop(&mut self) {
        if !self.released {
            self.owner.unpin(self.start);
            self.released = true;
        }
    }
}
