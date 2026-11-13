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

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use parking_lot::Mutex;
use thiserror::Error;
use crate::runtime_limits::{ScanControl, ScanMeter, ResourceError, HistoryLimits, HistoryUsage};

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
const MAX_SAVEPOINTS: usize = 16;
const MAX_READ_KEYS: usize = 4096;

#[derive(Debug, Error)]
pub enum DurableMvccError {
    #[error(transparent)]
    Resource(#[from] ResourceError),
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
    #[error("newer committed version conflicts with a transaction point read")]
    ReadConflict,
    #[error("a full-domain observation has an outdated global sequence")]
    ScanConflict,
    #[error("snapshot LSN {requested} is outside the retained range {floor}..={latest}")]
    SnapshotUnavailable { requested: u64, floor: u64, latest: u64 },
    #[error("cannot advance retention past an active pinned snapshot")]
    PinnedSnapshot,
    #[error("invalid savepoint operation: {0}")]
    Savepoint(String),
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
    usage: HistoryUsage,
}

fn state_stats(state: &State) -> DurableMvccStats {
    DurableMvccStats {
        latest_lsn: state.latest, retention_floor: state.floor,
        active_snapshots: state.pins.values().sum(),
        persisted_versions: state.usage.versions,
        known_keys: state.versions.len(),
    }
}

pub struct DurableMvccStore {
    rows: RowStore,
    state: Mutex<State>,
    limits: HistoryLimits,
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

fn decode_version(raw: &[u8]) -> DurableMvccResult<Option<&[u8]>> {
    match raw {
        [0] => Ok(None),
        [1, payload @ ..] if payload.len() <= MAX_USER_VALUE => Ok(Some(payload)),
        _ => Err(DurableMvccError::Corruption("invalid versioned value tag/size".into())),
    }
}

fn key_cost(key: &[u8]) -> usize { key.len() + std::mem::size_of::<(Vec<u8>, Vec<HistoricalVersion>)>() }
fn version_cost(key: &[u8], payload: Option<&[u8]>) -> usize {
    let payload = payload.map_or(0, <[u8]>::len);
    std::mem::size_of::<HistoricalVersion>() + payload + 14 + key.len() + 1 + payload
        + std::mem::size_of::<(Vec<u8>, Vec<u8>)>()
}
fn admit_version(mut usage: HistoryUsage, limits: HistoryLimits, key: &[u8], payload: Option<&[u8]>, new_key: bool) -> Result<HistoryUsage, ResourceError> {
    if new_key {
        usage.keys = usage.keys.checked_add(1).ok_or(ResourceError::HistoryLimit { resource:"keys",limit:limits.max_keys })?;
        usage.bytes = usage.bytes.checked_add(key_cost(key)).ok_or(ResourceError::HistoryLimit { resource:"bytes",limit:limits.max_bytes })?;
    }
    usage.versions = usage.versions.checked_add(1).ok_or(ResourceError::HistoryLimit { resource:"versions",limit:limits.max_versions })?;
    usage.bytes = usage.bytes.checked_add(version_cost(key,payload)).ok_or(ResourceError::HistoryLimit { resource:"bytes",limit:limits.max_bytes })?;
    limits.check(usage)?;
    Ok(usage)
}
fn projected_usage<'a>(state: &State, limits: HistoryLimits, entries: impl IntoIterator<Item=(&'a [u8],Option<&'a [u8]>)>) -> Result<HistoryUsage, ResourceError> {
    let mut usage = state.usage;
    let mut new_keys = BTreeSet::new();
    for (key,payload) in entries {
        let new_key = !state.versions.contains_key(key) && new_keys.insert(key);
        usage = admit_version(usage,limits,key,payload,new_key)?;
    }
    Ok(usage)
}

fn visible(versions: &[HistoricalVersion], horizon: u64) -> Option<Vec<u8>> {
    versions.iter().rev().find(|v| v.lsn <= horizon).and_then(|v| v.value.clone())
}

fn visible_checked<'a>(versions: &'a [HistoricalVersion], horizon: u64, meter: &mut ScanMeter<'_>) -> Result<Option<&'a [u8]>, ResourceError> {
    for version in versions.iter().rev() {
        meter.step()?;
        if version.lsn <= horizon { return Ok(version.value.as_deref()); }
    }
    Ok(None)
}

fn collect_visible(out: &mut Vec<(Vec<u8>, Vec<u8>)>, key: &[u8], value: Option<&[u8]>, meter: &mut ScanMeter<'_>) -> Result<(), ResourceError> {
    if let Some(value) = value {
        let bytes = key.len().saturating_add(value.len()).saturating_add(std::mem::size_of::<(Vec<u8>, Vec<u8>)>());
        meter.row(bytes)?;
        out.push((key.to_vec(), value.to_vec()));
    }
    Ok(())
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
        Self::open_with_limits(path, HistoryLimits::default())
    }

    pub fn open_with_limits(path: impl AsRef<Path>, limits: HistoryLimits) -> DurableMvccResult<Self> {
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
        let mut usage = HistoryUsage::default();
        let mut floor = None;
        let mut versions: BTreeMap<Vec<u8>, Vec<HistoricalVersion>> = BTreeMap::new();
        let latest = rows.visit_snapshot(|latest, key, value| -> DurableMvccResult<()> {
            if key == FORMAT_KEY {
                if value != FORMAT {
                    return Err(DurableMvccError::Corruption("invalid format marker".into()));
                }
                return Ok(());
            }
            if key == FLOOR_KEY {
                if value.len() != 8 {
                    return Err(DurableMvccError::Corruption("invalid retention floor".into()));
                }
                floor = Some(u64::from_be_bytes(value.try_into().unwrap()));
                return Ok(());
            }
            let (user_key, lsn) = parse_history_key(key)?;
            if lsn <= 1 || lsn > latest {
                return Err(DurableMvccError::Corruption("history version LSN out of range".into()));
            }
            let payload = decode_version(value)?;
            usage = admit_version(usage, limits, &user_key, payload, !versions.contains_key(&user_key))?;
            versions.entry(user_key).or_default().push(HistoricalVersion { lsn, value: payload.map(<[u8]>::to_vec) });
            Ok(())
        })?;
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
            rows, limits,
            state: Mutex::new(State { latest, floor, versions, pins: BTreeMap::new(), usage }),
        })
    }

    pub fn history_usage(&self) -> HistoryUsage { self.state.lock().usage }
    pub fn history_limits(&self) -> HistoryLimits { self.limits }

    pub(crate) fn check_additional_versions(&self, entries: &[(&[u8],Option<&[u8]>)]) -> DurableMvccResult<()> {
        for (key,payload) in entries { validate_user(key,*payload)?; }
        projected_usage(&self.state.lock(), self.limits, entries.iter().copied())?;
        Ok(())
    }

    /// G10 captures the exact version-key RowStore, including the retention
    /// floor, commit-tagged rows and tombstones. It does not transfer active
    /// in-process transaction pins into the restored process.
    pub fn backup_to(&self, destination: impl AsRef<Path>) -> DurableMvccResult<BackupReceipt> {
        Ok(self.backup_with_stats(destination)?.0)
    }

    /// Capture archive bytes and their describing statistics under one mutation lock.
    pub fn backup_with_stats(
        &self, destination: impl AsRef<Path>,
    ) -> DurableMvccResult<(BackupReceipt, DurableMvccStats)> {
        let state = self.state.lock();
        if state.latest != self.rows.last_lsn() {
            return Err(DurableMvccError::Corruption("WAL changed outside durable MVCC".into()));
        }
        let receipt = self.rows.backup_to(destination)?;
        Ok((receipt, state_stats(&state)))
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

    /// Prefix-bounded metadata inspection avoids copying unrelated row histories.
    /// Prefixes used by the caller must be disjoint; output keeps journal order.
    pub(crate) fn retained_versions_for_prefixes(
        &self, prefixes: &[&[u8]],
    ) -> Vec<(Vec<u8>, u64, Option<Vec<u8>>)> {
        let state = self.state.lock();
        let mut result = Vec::new();
        for prefix in prefixes {
            for (key, chain) in state.versions.range(prefix.to_vec()..) {
                if !key.starts_with(prefix) { break; }
                result.extend(chain.iter().map(|version|
                    (key.clone(), version.lsn, version.value.clone())));
            }
        }
        result.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        result
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

    /// Bound copied rows/bytes and examined keys/versions to a logical key prefix.
    pub fn scan_prefix_as_of_with_control(&self, prefix: &[u8], lsn: u64, control: &ScanControl) -> DurableMvccResult<Vec<(Vec<u8>, Vec<u8>)>> {
        if prefix.len() > MAX_USER_KEY { return Err(DurableMvccError::Capacity); }
        let mut meter = control.meter()?;
        let state = self.state.lock();
        control.check()?;
        if lsn < state.floor || lsn > state.latest {
            return Err(DurableMvccError::SnapshotUnavailable { requested: lsn, floor: state.floor, latest: state.latest });
        }
        let mut result = Vec::new();
        for (key, chain) in state.versions.range(prefix.to_vec()..).take_while(|(key, _)| key.starts_with(prefix)) {
            meter.step()?;
            let value = visible_checked(chain, lsn, &mut meter)?;
            collect_visible(&mut result, key, value, &mut meter)?;
        }
        control.check()?;
        Ok(result)
    }

    pub fn begin(&self) -> DurableMvccTransaction<'_> {
        self.begin_mode(false)
    }

    /// Validate observed point reads and scans atomically with commit publication.
    pub fn begin_with_read_validation(&self) -> DurableMvccTransaction<'_> {
        self.begin_mode(true)
    }

    fn begin_mode(&self, validate_reads: bool) -> DurableMvccTransaction<'_> {
        let mut state = self.state.lock();
        let start = state.latest;
        *state.pins.entry(start).or_default() += 1;
        DurableMvccTransaction {
            owner: self, start, pending: BTreeMap::new(), savepoints: Vec::new(), scanned: false, released: false,
            validate_reads, observations: Mutex::new(ReadObservations::default()),
        }
    }

    pub fn stats(&self) -> DurableMvccStats {
        let state = self.state.lock();
        state_stats(&state)
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
        let mut removed = 0;
        let mut cursor = Vec::new();
        loop {
            let mut doomed = Vec::new();
            for (key, chain) in state.versions.range(cursor.clone()..) {
                cursor = key.clone();
                let obsolete = chain.partition_point(|v| v.lsn <= floor).saturating_sub(1);
                for version in chain.iter().take(obsolete).take(GC_BATCH - doomed.len()) {
                    doomed.push((key.clone(),version.lsn));
                }
                if doomed.len() == GC_BATCH { break; }
            }
            if doomed.is_empty() { break; }
            let deletes = doomed.iter().map(|(key,lsn)| RowMutation::Delete { key: history_key(key,*lsn) }).collect::<Vec<_>>();
            let lsn = self.rows.commit_batch(Some(state.latest), &deletes)?;
            state.latest = lsn;
            let mut counts = BTreeMap::<Vec<u8>,usize>::new();
            for (key,_) in &doomed { *counts.entry(key.clone()).or_default() += 1; }
            let State { versions, usage, .. } = &mut *state;
            for (key,count) in counts {
                let chain = versions.get_mut(&key).expect("pruned key exists");
                for version in chain.drain(..count) {
                    usage.versions -= 1;
                    usage.bytes -= version_cost(&key,version.value.as_deref());
                }
            }
            removed += doomed.len();
        }
        // Do not keep large, now-unused version-vector reservations after
        // refunding live-version charges. Allocator RSS reclamation is separate.
        for chain in state.versions.values_mut() { chain.shrink_to_fit(); }
        Ok(removed)
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

#[derive(Default)]
struct ReadObservations {
    keys: BTreeSet<Vec<u8>>,
    whole_domain: bool,
}

struct Savepoint {
    name: String,
    pending: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
}

pub struct DurableMvccTransaction<'a> {
    owner: &'a DurableMvccStore,
    start: u64,
    pending: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    savepoints: Vec<Savepoint>,
    scanned: bool,
    released: bool,
    validate_reads: bool,
    observations: Mutex<ReadObservations>,
}

impl DurableMvccTransaction<'_> {
    pub fn start_lsn(&self) -> u64 { self.start }
    pub fn read_validation_enabled(&self) -> bool { self.validate_reads }

    fn observe_key(&self, key: &[u8]) {
        if !self.validate_reads { return; }
        let mut reads = self.observations.lock();
        if reads.whole_domain || reads.keys.contains(key) { return; }
        if reads.keys.len() == MAX_READ_KEYS {
            reads.whole_domain = true;
            reads.keys.clear();
        } else { reads.keys.insert(key.to_vec()); }
    }

    pub fn savepoint_count(&self) -> usize { self.savepoints.len() }

    /// Capture bounded staged mutations without changing the snapshot or WAL.
    pub fn savepoint(&mut self, name: &str) -> DurableMvccResult<()> {
        let mut chars = name.bytes();
        if name.len() > 63 || !chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
            || !chars.all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            return Err(DurableMvccError::Savepoint("name must be an ASCII identifier of 1..=63 bytes".into()));
        }
        if self.savepoints.len() >= MAX_SAVEPOINTS || self.pending.len() > MAX_WRITE_KEYS {
            return Err(DurableMvccError::Savepoint("bounded marker or staged-key capacity exceeded".into()));
        }
        self.savepoints.push(Savepoint { name: name.into(), pending: self.pending.clone() });
        Ok(())
    }

    fn savepoint_position(&self, name: &str) -> DurableMvccResult<usize> {
        self.savepoints.iter().rposition(|point| point.name == name)
            .ok_or_else(|| DurableMvccError::Savepoint("name does not exist".into()))
    }

    /// Restore staged writes, keeping the target marker and all conflict observations.
    pub fn rollback_to(&mut self, name: &str) -> DurableMvccResult<()> {
        let position = self.savepoint_position(name)?;
        self.pending = self.savepoints[position].pending.clone();
        self.savepoints.truncate(position + 1);
        Ok(())
    }

    /// Forget the target marker and its descendants while retaining staged writes.
    pub fn release_savepoint(&mut self, name: &str) -> DurableMvccResult<()> {
        let position = self.savepoint_position(name)?;
        self.savepoints.truncate(position);
        Ok(())
    }


    pub fn get(&self, key: &[u8]) -> DurableMvccResult<Option<Vec<u8>>> {
        validate_user(key, None)?;
        self.observe_key(key);
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

    /// Prefix-scoped ordered merge; no whole-domain or staged-map clone.
    pub fn scan_prefix_with_control(&mut self, prefix: &[u8], control: &ScanControl) -> DurableMvccResult<Vec<(Vec<u8>, Vec<u8>)>> {
        // Keep this observation even when a budget or cancellation aborts the scan.
        self.scanned = true;
        if prefix.len() > MAX_USER_KEY { return Err(DurableMvccError::Capacity); }
        let mut meter = control.meter()?;
        let state = self.owner.state.lock();
        control.check()?;
        let mut committed = state.versions.range(prefix.to_vec()..).take_while(|(key, _)| key.starts_with(prefix)).peekable();
        let mut pending = self.pending.range(prefix.to_vec()..).take_while(|(key, _)| key.starts_with(prefix)).peekable();
        let mut result = Vec::new();
        while committed.peek().is_some() || pending.peek().is_some() {
            meter.step()?;
            let use_pending = match (committed.peek(), pending.peek()) {
                (Some((a, _)), Some((b, _))) => b <= a,
                (None, Some(_)) => true,
                _ => false,
            };
            if use_pending {
                let (key, value) = pending.next().expect("pending key exists");
                if committed.peek().is_some_and(|(other, _)| *other == key) { committed.next(); }
                collect_visible(&mut result, key, value.as_deref(), &mut meter)?;
            } else {
                let (key, chain) = committed.next().expect("committed key exists");
                let value = visible_checked(chain, self.start, &mut meter)?;
                collect_visible(&mut result, key, value, &mut meter)?;
            }
        }
        control.check()?;
        Ok(result)
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
        if self.pending.is_empty() && !self.validate_reads { return Ok(self.start); }
        if self.pending.len() > MAX_WRITE_KEYS { return Err(DurableMvccError::CapacityExhausted); }
        let mut state = self.owner.state.lock();
        if self.scanned && state.latest != self.start {
            return Err(DurableMvccError::ScanConflict);
        }
        if self.validate_reads {
            let reads = self.observations.lock();
            if reads.whole_domain && state.latest != self.start {
                return Err(DurableMvccError::ScanConflict);
            }
            if reads.keys.iter().any(|key| state.versions.get(key).and_then(|vs| vs.last())
                .is_some_and(|v| v.lsn > self.start)) {
                return Err(DurableMvccError::ReadConflict);
            }
        }
        if self.pending.is_empty() { return Ok(self.start); }
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
        let usage = projected_usage(&state, self.owner.limits, self.pending.iter().map(|(key,value)|(key.as_slice(),value.as_deref())))?;
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
        state.usage = usage;
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

#[cfg(test)]
mod admission_tests {
    use super::*;
    #[test]
    fn retained_usage_overflow_is_a_resource_error_not_wrapped_admission() {
        let limits=HistoryLimits { max_keys:usize::MAX,max_versions:usize::MAX,max_bytes:usize::MAX };
        for (usage,new_key,resource) in [
            (HistoryUsage { keys:usize::MAX,versions:0,bytes:0 },true,"keys"),
            (HistoryUsage { keys:1,versions:usize::MAX,bytes:0 },false,"versions"),
            (HistoryUsage { keys:1,versions:0,bytes:usize::MAX },false,"bytes"),
        ] {
            assert!(matches!(admit_version(usage,limits,b"a",Some(b"v"),new_key),Err(ResourceError::HistoryLimit { resource:actual,.. }) if actual==resource));
        }
    }
}
