//! Single-writer durable key/value row storage for Bradian's relational kernel.
//!
//! The v1 protocol implements individually atomic INSERT/REPLACE and DELETE
//! mutations, ordered by a monotonically increasing LSN. Every successful write
//! is synced to the WAL before becoming visible in memory. A checkpoint publishes
//! validated BRPG pages before truncating the WAL. Recovery replays complete,
//! checksummed frames; only an incomplete final frame is removed.
//!
//! IMPORTANT: this is a durable *single-process* storage slice, not SQL, MVCC,
//! general SQL transactions, in-place mutable B+tree updates, or MVCC.
//! A real immutable disk B+tree is rebuilt from checkpointed rows and is
//! consulted for fully checkpointed point and range reads.
//! An OS-level exclusive lock prevents another cooperating RowStore instance
//! from opening the same directory while this instance remains alive.
//!
//! Files: rows.lock, rows.wal, snapshot-<lsn>.brp, snapshot-<lsn>.tmp,
//! index-<lsn>.bri and index-<lsn>.tmp.
//! Snapshots use the Page codec's 4096-byte physical pages and CRC integrity.
//! On Unix, directory sync seals the rename. Windows/macOS power-loss guarantees
//! require separate platform qualification; no claims are made before those tests.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use parking_lot::Mutex;

use crc32fast::Hasher;
use thiserror::Error;

use crate::runtime_limits::{ResourceError, RowStoreLimits, RowStoreUsage, IndexLimits, IndexBuildLimits, StorageLimits};
use crate::page::{Page, PageError, PageKind, MAX_PAYLOAD_SIZE, PAGE_SIZE};
use crate::btree::{DiskBTree, IndexError, RowPointer};
use crate::buffer_pool::{CacheError, CacheStats, PageCache};
use crate::index_recovery::{self, IndexRecovery};
use crate::checkpoint_manifest::{self, CheckpointManifest, ManifestError};
use crate::backup::{self, BackupReceipt};

/// Consistent owned binary rows returned by a storage snapshot.
pub type RowSnapshot = Vec<(Vec<u8>, Vec<u8>)>;
type RowMap = BTreeMap<Vec<u8>, Vec<u8>>;

const WAL_MAGIC: &[u8; 4] = b"BRWL";
const WAL_HEADER: usize = 12;
const ROW_HEADER: usize = 6;
const MAX_KEY: usize = 512;
const MAX_VALUE: usize = MAX_PAYLOAD_SIZE - ROW_HEADER - MAX_KEY;
const META_LENGTH: usize = 24;
const MAX_WAL_BODY: usize = 1024 * 1024;
const MAX_BATCH_OPS: usize = 256;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error(transparent)]
    Resource(#[from] ResourceError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Page(#[from] PageError),
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error(transparent)]
    Cache(#[from] CacheError),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("invalid row: {0}")]
    InvalidRow(&'static str),
    #[error("corrupt row store: {0}")]
    Corruption(String),
    #[error("store is poisoned after a failed write or checkpoint; reopen after investigation")]
    Poisoned,
    #[error("monotonic row-log sequence exhausted")]
    SequenceExhausted,
    #[error("row-store directory is already open for writing in another process")]
    Busy,
    #[error("transaction conflict: expected committed LSN {expected}, observed {actual}")]
    Conflict { expected: u64, actual: u64 },
}

pub type StoreResult<T> = Result<T, StoreError>;

/// One operation within an atomically committed, checksummed WAL batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowMutation {
    Put { key: Vec<u8>, value: Vec<u8> },
    Delete { key: Vec<u8> },
}

#[derive(Debug)]
struct Inner {
    rows: BTreeMap<Vec<u8>, Vec<u8>>,
    // Committed changes since the published checkpoint. None is a tombstone.
    // Recovery reconstructs this overlay from synchronized WAL frames.
    delta: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    wal: File,
    lsn: u64,
    checkpoint_lsn: u64,
    index: Option<DiskBTree>,
    snapshot_pages: Option<PageCache>,
    poisoned: bool,
    usage: RowStoreUsage,
}

/// Durable, single-process row store. A mutex serializes operations within this
/// instance; an OS advisory lock restricts competing cooperating processes.
#[derive(Debug)]
pub struct RowStore {
    root: PathBuf,
    limits: RowStoreLimits,
    index_limits: IndexLimits,
    build_limits: IndexBuildLimits,
    inner: Mutex<Inner>,
    // Drop after the WAL handle so the exclusive ownership remains in force
    // throughout teardown. The kernel releases the lock automatically.
    _lock: File,
}

impl RowStore {
    pub fn open(root: impl AsRef<Path>) -> StoreResult<Self> {
        Self::open_with_limits(root, RowStoreLimits::default())
    }

    pub fn open_with_limits(root: impl AsRef<Path>, limits: RowStoreLimits) -> StoreResult<Self> {
        Self::open_with_recovery_limits(root, limits, IndexLimits::default())
    }

    pub fn open_with_recovery_limits(root: impl AsRef<Path>, limits: RowStoreLimits, index_limits: IndexLimits) -> StoreResult<Self> {
        Self::open_with_policies(root, StorageLimits { rows:limits, index:index_limits, ..StorageLimits::default() })
    }

    pub fn open_with_policies(root: impl AsRef<Path>, policies: StorageLimits) -> StoreResult<Self> {
        let StorageLimits { rows:limits, index:index_limits, build:build_limits } = policies;
        let root = root.as_ref();
        fs::create_dir_all(root)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("rows.lock"))?;
        match lock.try_lock() {
            Ok(()) => {},
            Err(TryLockError::WouldBlock) => return Err(StoreError::Busy),
            Err(TryLockError::Error(error)) => return Err(StoreError::Io(error)),
        }
        let (checkpoint_lsn, mut rows, mut usage) = read_latest_snapshot(root, limits)?;
        let mut delta = BTreeMap::new();
        let wal_path = root.join("rows.wal");
        let mut wal = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(wal_path)?;
        // Persist the directory entry for newly created WAL files on Unix.
        sync_directory(root)?;
        let (lsn, valid_length) = replay(&mut wal, checkpoint_lsn, &mut rows, &mut delta, &mut usage, limits)?;
        // A committed checkpoint can outlive an interrupted derived-index
        // publication. Rebuild only an absent index from verified pages.
        if snapshot_path(root, checkpoint_lsn).exists() {
            if !index_recovery::index_path(root, checkpoint_lsn).exists() {
                index_recovery::rebuild_missing_with_limits(root, checkpoint_lsn, index_limits, build_limits)?;
            }
            if let Some(manifest) = checkpoint_manifest::load(root)? {
                checkpoint_manifest::check_index(root, manifest)?;
            }
        }
        // An interrupted append is not a complete transaction. Repair the
        // trailing, incomplete frame so future writes remain replayable.
        if valid_length < wal.metadata()?.len() {
            wal.set_len(valid_length)?;
            wal.sync_all()?;
        }
        wal.seek(SeekFrom::End(0))?;
        Ok(Self {
            root: root.to_owned(),
            limits,
            index_limits,
            build_limits,
            _lock: lock,
            inner: Mutex::new(Inner {
                rows,
                delta,
                wal,
                lsn,
                checkpoint_lsn,
                index: None,
                snapshot_pages: None,
                poisoned: false,
                usage,
            }),
        })
    }

    pub fn resource_usage(&self) -> RowStoreUsage { self.inner.lock().usage }
    pub fn resource_limits(&self) -> RowStoreLimits { self.limits }
    pub fn storage_limits(&self) -> StorageLimits { StorageLimits { rows:self.limits,index:self.index_limits,build:self.build_limits } }

    pub fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.inner.lock().rows.get(key).cloned()
    }

    /// Return an ordered, owned snapshot of the current rows. Not an MVCC snapshot.
    pub fn scan(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.inner.lock()
            .rows.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// Internal recovery traversal: the callback must not re-enter this store.
    pub(crate) fn visit_snapshot<E>(
        &self, mut visit: impl FnMut(u64, &[u8], &[u8]) -> Result<(), E>,
    ) -> Result<u64, E> {
        let inner = self.inner.lock();
        for (key, value) in &inner.rows { visit(inner.lsn, key, value)?; }
        Ok(inner.lsn)
    }

    /// Capture a bounded raw image and its LSN under one lock. Only the
    /// explicitly named metadata record is excluded; all other keys survive
    /// for the typed caller's integrity checks.
    pub fn snapshot_with_control(
        &self, exclude_key: Option<&[u8]>, control: &crate::runtime_limits::ScanControl,
    ) -> Result<(u64, RowSnapshot), crate::runtime_limits::ResourceError> {
        let mut meter = control.meter()?;
        let inner = self.inner.lock();
        control.check()?;
        let mut rows = Vec::new();
        for (key, value) in &inner.rows {
            meter.step()?;
            if exclude_key == Some(key.as_slice()) { continue; }
            let bytes = key.len().saturating_add(value.len()).saturating_add(std::mem::size_of::<(Vec<u8>, Vec<u8>)>());
            meter.row(bytes)?;
            rows.push((key.clone(), value.clone()));
        }
        control.check()?;
        Ok((inner.lsn, rows))
    }

    /// Obtain one consistent row image and its committed WAL position under
    /// a single lock. Transaction validation must never read the LSN and
    /// snapshot in separate lock acquisitions.
    pub fn snapshot_with_lsn(&self) -> (u64, RowSnapshot) {
        let inner = self.inner.lock();
        (inner.lsn, inner.rows.iter().map(|(key, value)| (key.clone(), value.clone())).collect())
    }

    pub fn len(&self) -> usize {
        self.inner.lock().rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn last_lsn(&self) -> u64 {
        self.inner.lock().lsn
    }

    /// Create a verified, self-contained backup of one recoverable committed
    /// state. First establish a checkpoint, then hold the mutation mutex while
    /// copying the checkpoint manifest, physical snapshot, B+tree and all newer
    /// committed WAL records. Concurrent writes cannot change the captured
    /// LSN during the copy. The backup appears at the destination only after
    /// its recovered contents and every index pointer have been validated.
    pub fn backup_to(&self, destination: impl AsRef<Path>) -> StoreResult<BackupReceipt> {
        self.checkpoint()?;
        self.backup_current_generation(destination)
    }

    /// Preserve the latest committed checkpoint *and* the synchronized WAL
    /// appended after it, without forcing another checkpoint. The mutation
    /// mutex freezes both physical sources at a single acknowledged LSN.
    /// This is suitable for repeated backups of a busy local RowStore.
    pub fn backup_current_generation(
        &self,
        destination: impl AsRef<Path>,
    ) -> StoreResult<BackupReceipt> {
        let inner = self.inner.lock();
        if inner.poisoned {
            return Err(StoreError::Poisoned);
        }
        inner.wal.sync_all()?;
        let receipt = BackupReceipt {
            checkpoint_lsn: inner.checkpoint_lsn,
            last_lsn: inner.lsn,
            rows: inner.rows.len() as u64,
        };
        backup::create_backup(&self.root, destination.as_ref(), receipt, &inner.rows, self.storage_limits())
    }

    /// Restore a verified physical database backup into an absent directory.
    /// An existing database is never overwritten.
    pub fn restore_backup(
        backup: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> StoreResult<BackupReceipt> {
        Self::restore_backup_with_limits(backup, destination, RowStoreLimits::default())
    }

    pub fn restore_backup_with_limits(
        backup: impl AsRef<Path>, destination: impl AsRef<Path>, limits: RowStoreLimits,
    ) -> StoreResult<BackupReceipt> {
        Self::restore_backup_with_recovery_limits(backup, destination, limits, IndexLimits::default())
    }

    pub fn restore_backup_with_recovery_limits(
        backup: impl AsRef<Path>, destination: impl AsRef<Path>, limits: RowStoreLimits, index_limits: IndexLimits,
    ) -> StoreResult<BackupReceipt> {
        Self::restore_backup_with_policies(backup, destination, StorageLimits { rows:limits,index:index_limits,..StorageLimits::default() })
    }

    pub fn restore_backup_with_policies(
        backup: impl AsRef<Path>, destination: impl AsRef<Path>, policies: StorageLimits,
    ) -> StoreResult<BackupReceipt> {
        backup::restore_backup_with_policies(backup.as_ref(), destination.as_ref(), policies)
    }

    /// Sync exactly one replace/insert operation before publishing it.
    pub fn put(&self, key: &[u8], value: &[u8]) -> StoreResult<u64> {
        validate_row(key, value)?;
        self.mutate(key, Some(value))
    }

    /// A delete is durably logged even when the key did not previously exist.
    pub fn delete(&self, key: &[u8]) -> StoreResult<u64> {
        validate_key(key)?;
        self.mutate(key, None)
    }

    fn mutate(&self, key: &[u8], value: Option<&[u8]>) -> StoreResult<u64> {
        let mut inner = self.inner.lock();
        if inner.poisoned {
            return Err(StoreError::Poisoned);
        }
        let changes = BTreeMap::from([(key,value)]);
        let usage = projected_usage(&inner.rows, &inner.delta, inner.usage, self.limits, &changes)?;
        let next = inner.lsn.checked_add(1).ok_or(StoreError::SequenceExhausted)?;
        let frame = encode_wal(next, key, value);
        // Any WAL write/sync failure can leave an unknown partial record. The
        // process must stop issuing writes until the log is recovered.
        if let Err(error) = inner.wal.write_all(&frame).and_then(|_| inner.wal.sync_all()) {
            inner.poisoned = true;
            return Err(StoreError::Io(error));
        }
        let Inner { rows, delta, .. } = &mut *inner;
        apply_changes(rows, delta, &changes);
        inner.usage = usage;
        inner.lsn = next;
        Ok(next)
    }

    /// Atomically commit several row mutations under one durable WAL frame.
    ///
    /// All validation occurs before the WAL is modified. Once the frame is
    /// synchronized, every operation becomes visible under one in-process
    /// mutex acquisition. An incomplete batch is never replayed. Optional
    /// optimistic validation prevents commits based on an outdated global LSN.
    ///
    /// This is not MVCC or a general SQL transaction: it provides atomic
    /// batch updates and serialized commit order for one local RowStore.
    pub fn commit_batch(
        &self,
        expected_lsn: Option<u64>,
        mutations: &[RowMutation],
    ) -> StoreResult<u64> {
        if mutations.is_empty() || mutations.len() > MAX_BATCH_OPS {
            return Err(StoreError::InvalidRow("transaction needs 1..=256 operations"));
        }
        let mut size = 13_usize; // LSN + opcode + operation count
        for mutation in mutations {
            let (key, value) = match mutation {
                RowMutation::Put { key, value } => {
                    validate_row(key, value)?;
                    (key.as_slice(), value.as_slice())
                }
                RowMutation::Delete { key } => {
                    validate_key(key)?;
                    (key.as_slice(), &[][..])
                }
            };
            size = size.checked_add(7 + key.len() + value.len())
                .ok_or(StoreError::InvalidRow("transaction WAL frame exceeds limit"))?;
            if size > MAX_WAL_BODY {
                return Err(StoreError::InvalidRow("transaction WAL frame exceeds limit"));
            }
        }

        let mut inner = self.inner.lock();
        if inner.poisoned {
            return Err(StoreError::Poisoned);
        }
        if let Some(expected) = expected_lsn {
            if expected != inner.lsn {
                return Err(StoreError::Conflict { expected, actual: inner.lsn });
            }
        }
        let changes = final_changes(mutations);
        let usage = projected_usage(&inner.rows, &inner.delta, inner.usage, self.limits, &changes)?;
        let next = inner.lsn.checked_add(1).ok_or(StoreError::SequenceExhausted)?;
        let frame = encode_batch(next, mutations, size);
        if let Err(error) = inner.wal.write_all(&frame).and_then(|_| inner.wal.sync_all()) {
            inner.poisoned = true;
            return Err(StoreError::Io(error));
        }
        let Inner { rows, delta, .. } = &mut *inner;
        apply_changes(rows, delta, &changes);
        inner.usage = usage;
        inner.lsn = next;
        Ok(next)
    }

    /// On-disk indexed read of a fully checkpointed key. When WAL mutations
    /// are newer than the checkpoint, the in-memory committed map is used
    /// instead so callers never observe a stale checkpoint.
    ///
    /// This method exposes checksum/index corruption as an error rather than
    /// silently treating a damaged page as an absent row.
    /// Query a mutable logical index consisting of a committed WAL delta over
    /// an immutable disk B+tree. Only keys changed after the last checkpoint
    /// are stored in the delta; unchanged keys are resolved through disk pages.
    pub fn indexed_get(&self, key: &[u8]) -> StoreResult<Option<Vec<u8>>> {
        let mut inner = self.inner.lock();
        let expected = inner.rows.get(key).cloned();
        if let Some(changed) = inner.delta.get(key) {
            // Tombstones are explicit: Some(None) means deleted.
            if changed.as_ref() != expected.as_ref() {
                return Err(StoreError::Corruption("WAL delta disagrees with committed row map".into()));
            }
            return Ok(changed.clone());
        }
        let checkpoint = inner.checkpoint_lsn;
        let index_path = self.root.join(format!("index-{checkpoint}.bri"));
        if !index_path.exists() {
            // No published index yet (or legacy checkpoint). The full row map
            // remains authoritative and is always WAL-recoverable.
            return Ok(expected);
        }
        if inner.index.is_none() {
            inner.index = Some(DiskBTree::open(&index_path, checkpoint)?);
        }
        let pointer = inner.index.as_mut().expect("opened index").get(key)?;
        let result = if let Some(pointer) = pointer {
            if inner.snapshot_pages.is_none() {
                let path = self.root.join(format!("snapshot-{checkpoint}.brp"));
                inner.snapshot_pages = Some(PageCache::open(&path, checkpoint, 128)?);
            }
            Some(read_indexed_record(
                inner.snapshot_pages.as_mut().expect("opened snapshot"),
                key, pointer,
            )?)
        } else {
            None
        };
        if result != expected {
            return Err(StoreError::Corruption("indexed result differs from committed row map".into()));
        }
        Ok(result)
    }

    /// Range reads merge the immutable checkpoint B+tree with the synchronized
    /// WAL delta. Put replaces a checkpoint row, and delete is a tombstone.
    /// Every result is checked against authoritative committed rows.
    pub fn indexed_range(&self, start: &[u8], end: &[u8]) -> StoreResult<Vec<(Vec<u8>, Vec<u8>)>> {
        if start >= end { return Ok(Vec::new()); }
        let mut inner = self.inner.lock();
        let expected = inner.rows.range(start.to_vec()..end.to_vec())
            .map(|(k,v)| (k.clone(),v.clone())).collect::<Vec<_>>();
        let checkpoint = inner.checkpoint_lsn;
        let index_path = self.root.join(format!("index-{checkpoint}.bri"));
        if !index_path.exists() {
            return Ok(expected);
        }
        if inner.index.is_none() {
            inner.index = Some(DiskBTree::open(&index_path, checkpoint)?);
        }
        let entries = inner.index.as_mut().expect("opened index").range(start, end)?;
        if !entries.is_empty() && inner.snapshot_pages.is_none() {
            let path = self.root.join(format!("snapshot-{checkpoint}.brp"));
            inner.snapshot_pages = Some(PageCache::open(&path, checkpoint, 128)?);
        }
        let mut result = BTreeMap::new();
        for (key, pointer) in entries {
            let value = read_indexed_record(
                inner.snapshot_pages.as_mut().expect("nonempty indexed range"),
                &key, pointer,
            )?;
            result.insert(key, value);
        }
        for (key, value) in inner.delta.range(start.to_vec()..end.to_vec()) {
            match value {
                Some(value) => { result.insert(key.clone(), value.clone()); }
                None => { result.remove(key); }
            }
        }
        let merged = result.into_iter().collect::<Vec<_>>();
        if merged != expected {
            return Err(StoreError::Corruption("merged index/delta differs from committed row map".into()));
        }
        Ok(merged)
    }

    /// Current number of distinct keys changed since the latest checkpoint,
    /// including deleted-key tombstones.
    pub fn uncheckpointed_index_changes(&self) -> usize {
        self.inner.lock().delta.len()
    }

    /// Cache occupancy and hit/miss counters for the current physical generation.
    /// Caches are invalidated when a new checkpoint generation is published.
    pub fn index_cache_stats(&self) -> Option<(CacheStats, Option<CacheStats>)> {
        let inner = self.inner.lock();
        inner.index.as_ref().map(|idx| (
            idx.cache_stats(),
            inner.snapshot_pages.as_ref().map(PageCache::stats),
        ))
    }

    /// Verify every checkpoint index pointer against immutable row pages.
    /// Full verification is intentionally an explicit O(N log N) operation.
    pub fn verify_checkpoint_index(&self) -> StoreResult<IndexRecovery> {
        self.verify_checkpoint_index_with_limits(self.index_limits)
    }

    pub fn verify_checkpoint_index_with_limits(&self, limits: IndexLimits) -> StoreResult<IndexRecovery> {
        let inner = self.inner.lock();
        if !snapshot_path(&self.root, inner.checkpoint_lsn).exists() {
            return Err(StoreError::Corruption("no published checkpoint to verify".into()));
        }
        index_recovery::verify_with_limits(&self.root, inner.checkpoint_lsn, limits)
    }

    /// Reconstruct a missing derived index from the authoritative checkpoint.
    /// Existing published indexes, even damaged ones, are never overwritten.
    pub fn rebuild_missing_checkpoint_index(&self) -> StoreResult<IndexRecovery> {
        self.rebuild_missing_checkpoint_index_with_limits(self.index_limits)
    }

    pub fn rebuild_missing_checkpoint_index_with_limits(&self, limits: IndexLimits) -> StoreResult<IndexRecovery> {
        let mut inner = self.inner.lock();
        if !snapshot_path(&self.root, inner.checkpoint_lsn).exists() {
            return Err(StoreError::Corruption("no published checkpoint to rebuild".into()));
        }
        inner.index = None;
        inner.snapshot_pages = None;
        index_recovery::rebuild_missing_with_limits(&self.root, inner.checkpoint_lsn, limits, self.build_limits)
    }

    /// Publish a complete checkpoint and then truncate the WAL.
    ///
    /// Failure before WAL truncation leaves the old WAL intact. A successfully
    /// renamed snapshot is authoritative even if a crash precedes WAL truncation:
    /// replay ignores records at or below its committed checkpoint LSN.
    pub fn checkpoint(&self) -> StoreResult<u64> {
        self.checkpoint_with_limits(self.build_limits)
    }

    pub fn checkpoint_with_limits(&self, limits: IndexBuildLimits) -> StoreResult<u64> {
        let mut inner = self.inner.lock();
        if inner.poisoned {
            return Err(StoreError::Poisoned);
        }
        if inner.checkpoint_lsn == inner.lsn && snapshot_path(&self.root, inner.lsn).exists() {
            match checkpoint_manifest::load(&self.root)? {
                Some(current) if current.lsn == inner.lsn => return Ok(inner.lsn),
                Some(_) => {
                    return Err(StoreError::Corruption(
                        "checkpoint generation and committed manifest disagree".into(),
                    ));
                }
                None => {
                    // Upgrade a verified pre-manifest checkpoint *in place*:
                    // no payload bytes or WAL commits are changed.
                    let report = index_recovery::verify_with_limits(&self.root, inner.lsn, self.index_limits)?;
                    let snapshot_length = fs::metadata(snapshot_path(&self.root, inner.lsn))?.len();
                    let index_length = fs::metadata(index_recovery::index_path(&self.root, inner.lsn))?.len();
                    if snapshot_length % PAGE_SIZE as u64 != 0 || index_length % PAGE_SIZE as u64 != 0 {
                        return Err(StoreError::Corruption("legacy checkpoint is not page aligned".into()));
                    }
                    let manifest = CheckpointManifest {
                        lsn: inner.lsn,
                        rows: report.rows,
                        data_pages: report.data_pages,
                        index_pages: index_length / PAGE_SIZE as u64 - 1,
                    };
                    checkpoint_manifest::publish(&self.root, manifest)?;
                    return Ok(inner.lsn);
                }
            }
        }
        // Admission is read-only and precedes quarantine, temporary-file writes,
        // publication and WAL truncation. Refusal must not poison this owner.
        crate::btree::admit_build_keys(inner.rows.keys().map(Vec::len),limits)?;
        let lsn = inner.lsn;
        // The manifest is the publication boundary. A prior crashed attempt
        // may have left snapshot/index filenames for this same WAL LSN, but
        // those files were never committed as the active generation.
        if let Some(committed) = checkpoint_manifest::load(&self.root)? {
            checkpoint_manifest::quarantine_uncommitted(&self.root, committed, lsn)?;
        }
        let tmp = self.root.join(format!("snapshot-{lsn}.tmp"));
        let final_path = snapshot_path(&self.root, lsn);
        // A previous interrupted checkpoint may leave a .tmp. Truncating only
        // that file is safe: published .brp snapshots are never overwritten.
        let write_result = (|| -> StoreResult<()> {
            let mut file = OpenOptions::new().create(true).write(true).truncate(true).open(&tmp)?;
            let pointers = write_snapshot(&mut file, lsn, &inner.rows)?;
            file.sync_all()?;
            drop(file);
            // Refuse overwriting an existing published snapshot.
            if final_path.exists() {
                return Err(StoreError::Corruption("checkpoint identity already exists".into()));
            }
            fs::rename(&tmp, &final_path)?;
            sync_directory(&self.root)?;
            // The index is a derived accelerator. If index construction fails,
            // the WAL remains intact and restart can use the authoritative
            // published snapshot. Never truncate the WAL before this point.
            let index_tmp = self.root.join(format!("index-{lsn}.tmp"));
            let index_final = self.root.join(format!("index-{lsn}.bri"));
            if index_tmp.exists() { fs::remove_file(&index_tmp)?; }
            if index_final.exists() {
                return Err(StoreError::Corruption("checkpoint index identity already exists".into()));
            }
            DiskBTree::build_with_limits(&index_tmp, lsn, &pointers, limits)?;
            fs::rename(&index_tmp, &index_final)?;
            sync_directory(&self.root)?;
            let page_size = PAGE_SIZE as u64;
            let snapshot_pages = fs::metadata(&final_path)?.len() / page_size;
            let index_pages = fs::metadata(&index_final)?.len() / page_size;
            let manifest = CheckpointManifest {
                lsn,
                rows: inner.rows.len() as u64,
                data_pages: snapshot_pages.checked_sub(1)
                    .ok_or_else(|| StoreError::Corruption("empty snapshot file".into()))?,
                index_pages: index_pages.checked_sub(1)
                    .ok_or_else(|| StoreError::Corruption("empty index file".into()))?,
            };
            checkpoint_manifest::publish(&self.root, manifest)?;
            Ok(())
        })();
        if let Err(error) = write_result {
            inner.poisoned = true;
            return Err(error);
        }
        // Publish is durable before truncating the redundant WAL.
        if let Err(error) = inner.wal.set_len(0).and_then(|_| inner.wal.sync_all())
            .and_then(|_| inner.wal.seek(SeekFrom::Start(0)).map(|_| ()))
        {
            inner.poisoned = true;
            return Err(StoreError::Io(error));
        }
        inner.checkpoint_lsn = lsn;
        inner.delta.clear();
        inner.usage.delta_keys = 0;
        inner.usage.delta_bytes = 0;
        inner.index = None;
        inner.snapshot_pages = None;
        // Older checkpoints are no longer needed once the newest snapshot is
        // published and WAL truncation is complete. Cleanup is opportunistic.
        let _ = prune_snapshots(&self.root, lsn);
        Ok(lsn)
    }
}

type Changes<'a> = BTreeMap<&'a [u8], Option<&'a [u8]>>;
fn row_cost(key: &[u8], value: &[u8]) -> usize {
    std::mem::size_of::<(Vec<u8>, Vec<u8>)>() + key.len() + value.len()
}
fn delta_cost(key: &[u8], value: Option<&[u8]>) -> usize {
    std::mem::size_of::<(Vec<u8>, Option<Vec<u8>>)>() + key.len() + value.map_or(0, <[u8]>::len)
}
fn add_usage(current: usize, added: usize, resource: &'static str, limit: usize) -> Result<usize, ResourceError> {
    current.checked_add(added).ok_or(ResourceError::RowStoreLimit { resource, limit })
}
fn final_changes(mutations: &[RowMutation]) -> Changes<'_> {
    mutations.iter().map(|m| match m {
        RowMutation::Put { key, value } => (key.as_slice(),Some(value.as_slice())),
        RowMutation::Delete { key } => (key.as_slice(),None),
    }).collect()
}
fn projected_usage(rows: &RowMap, delta: &BTreeMap<Vec<u8>, Option<Vec<u8>>>, mut usage: RowStoreUsage,
    limits: RowStoreLimits, changes: &Changes<'_>) -> StoreResult<RowStoreUsage> {
    // Remove every affected old charge before adding the final coalesced state.
    // Admission must not depend on key order or intermediate operations in a batch.
    for key in changes.keys() {
        if let Some(value) = rows.get(*key) { usage.rows -= 1; usage.row_bytes -= row_cost(key,value); }
        if let Some(value) = delta.get(*key) { usage.delta_keys -= 1; usage.delta_bytes -= delta_cost(key,value.as_deref()); }
    }
    for (key,value) in changes {
        if let Some(value) = value {
            usage.rows = add_usage(usage.rows,1,"rows",limits.max_rows)?;
            usage.row_bytes = add_usage(usage.row_bytes,row_cost(key,value),"row bytes",limits.max_row_bytes)?;
        }
        usage.delta_keys = add_usage(usage.delta_keys,1,"delta keys",limits.max_delta_keys)?;
        usage.delta_bytes = add_usage(usage.delta_bytes,delta_cost(key,*value),"delta bytes",limits.max_delta_bytes)?;
    }
    limits.check(usage)?;
    Ok(usage)
}
fn apply_changes(rows: &mut RowMap, delta: &mut BTreeMap<Vec<u8>, Option<Vec<u8>>>, changes: &Changes<'_>) {
    // Drop affected old entries first so admitted final state is also the map-copy peak.
    for key in changes.keys() { rows.remove(*key); delta.remove(*key); }
    for (key,value) in changes {
        if let Some(value) = value { rows.insert(key.to_vec(),value.to_vec()); }
        delta.insert(key.to_vec(),value.map(<[u8]>::to_vec));
    }
}

fn validate_key(key: &[u8]) -> StoreResult<()> {
    if key.is_empty() || key.len() > MAX_KEY {
        return Err(StoreError::InvalidRow("key length must be 1..=512 bytes"));
    }
    Ok(())
}

fn validate_row(key: &[u8], value: &[u8]) -> StoreResult<()> {
    validate_key(key)?;
    if value.len() > MAX_VALUE {
        return Err(StoreError::InvalidRow("value exceeds inline row-page limit"));
    }
    Ok(())
}

fn checksum(bytes: &[u8]) -> u32 {
    let mut h = Hasher::new();
    h.update(bytes);
    h.finalize()
}

fn encode_frame(body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(WAL_HEADER + body.len());
    frame.extend_from_slice(WAL_MAGIC);
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&checksum(body).to_le_bytes());
    frame.extend_from_slice(body);
    frame
}

fn encode_wal(lsn: u64, key: &[u8], value: Option<&[u8]>) -> Vec<u8> {
    let value_len = value.map_or(0, |v| v.len());
    let mut body = Vec::with_capacity(8 + 1 + ROW_HEADER + key.len() + value_len);
    body.extend_from_slice(&lsn.to_le_bytes());
    body.push(if value.is_some() { 1 } else { 2 });
    body.extend_from_slice(&(key.len() as u16).to_le_bytes());
    body.extend_from_slice(&(value_len as u32).to_le_bytes());
    body.extend_from_slice(key);
    if let Some(value) = value {
        body.extend_from_slice(value);
    }
    encode_frame(&body)
}

fn encode_batch(lsn: u64, mutations: &[RowMutation], size: usize) -> Vec<u8> {
    let mut body = Vec::with_capacity(size);
    body.extend_from_slice(&lsn.to_le_bytes());
    body.push(3); // one CRC-protected atomic multi-operation commit
    body.extend_from_slice(&(mutations.len() as u32).to_le_bytes());
    for mutation in mutations {
        let (op, key, value) = match mutation {
            RowMutation::Put { key, value } => (1_u8, key.as_slice(), value.as_slice()),
            RowMutation::Delete { key } => (2_u8, key.as_slice(), &[][..]),
        };
        body.push(op);
        body.extend_from_slice(&(key.len() as u16).to_le_bytes());
        body.extend_from_slice(&(value.len() as u32).to_le_bytes());
        body.extend_from_slice(key);
        body.extend_from_slice(value);
    }
    encode_frame(&body)
}

fn decode_mutations(body: &[u8], offset: u64) -> StoreResult<Vec<RowMutation>> {
    let malformed = || StoreError::Corruption(format!("malformed WAL body at byte {offset}"));
    if body.len() < 13 {
        return Err(malformed());
    }
    let batch = body[8] == 3;
    let count = if batch {
        u32::from_le_bytes(body[9..13].try_into().unwrap()) as usize
    } else {
        1
    };
    if count == 0 || count > MAX_BATCH_OPS {
        return Err(malformed());
    }
    let mut pos = if batch { 13_usize } else { 8_usize };
    let mut operations = Vec::with_capacity(count);
    for _ in 0..count {
        if body.len().saturating_sub(pos) < 7 {
            return Err(malformed());
        }
        let op = body[pos];
        let key_len = u16::from_le_bytes(body[pos + 1..pos + 3].try_into().unwrap()) as usize;
        let value_len = u32::from_le_bytes(body[pos + 3..pos + 7].try_into().unwrap()) as usize;
        let record_size = 7_usize.checked_add(key_len)
            .and_then(|n| n.checked_add(value_len))
            .ok_or_else(malformed)?;
        if key_len == 0 || key_len > MAX_KEY || value_len > MAX_VALUE
            || record_size > body.len() - pos
            || (op == 2 && value_len != 0)
            || (op != 1 && op != 2)
        {
            return Err(malformed());
        }
        let key = body[pos + 7..pos + 7 + key_len].to_vec();
        let value = body[pos + 7 + key_len..pos + record_size].to_vec();
        operations.push(if op == 1 {
            RowMutation::Put { key, value }
        } else {
            RowMutation::Delete { key }
        });
        pos += record_size;
    }
    if pos != body.len() {
        return Err(malformed());
    }
    Ok(operations)
}

fn replay(
    wal: &mut File,
    checkpoint_lsn: u64,
    rows: &mut BTreeMap<Vec<u8>, Vec<u8>>,
    delta: &mut BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    usage: &mut RowStoreUsage, limits: RowStoreLimits,
) -> StoreResult<(u64, u64)> {
    wal.seek(SeekFrom::Start(0))?;
    let mut lsn = checkpoint_lsn;
    let mut previous_in_wal = None::<u64>;
    let mut valid_end = 0_u64;
    loop {
        let start = wal.stream_position()?;
        let mut header = [0_u8; WAL_HEADER];
        if let Err(err) = wal.read_exact(&mut header) {
            if err.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok((lsn, valid_end));
            }
            return Err(StoreError::Io(err));
        }
        if &header[..4] != WAL_MAGIC {
            return Err(StoreError::Corruption(format!("bad WAL magic at byte {start}")));
        }
        let len = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        if !(13..=MAX_WAL_BODY).contains(&len) {
            return Err(StoreError::Corruption(format!("invalid WAL frame length at byte {start}")));
        }
        let crc = u32::from_le_bytes(header[8..12].try_into().unwrap());
        let mut body = vec![0_u8; len];
        if let Err(err) = wal.read_exact(&mut body) {
            if err.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok((lsn, valid_end));
            }
            return Err(StoreError::Io(err));
        }
        if checksum(&body) != crc {
            return Err(StoreError::Corruption(format!("WAL CRC mismatch at byte {start}")));
        }
        let seq = u64::from_le_bytes(body[0..8].try_into().unwrap());
        if seq == 0 || previous_in_wal.is_some_and(|previous| previous.checked_add(1) != Some(seq)) {
            return Err(StoreError::Corruption(format!("WAL sequence discontinuity at byte {start}")));
        }
        previous_in_wal = Some(seq);
        // Decode and validate the entire batch before modifying any row.
        let operations = decode_mutations(&body, start)?;
        if seq > checkpoint_lsn {
            if seq != lsn.checked_add(1).ok_or(StoreError::SequenceExhausted)? {
                return Err(StoreError::Corruption(format!("WAL gap after checkpoint at byte {start}")));
            }
            let changes = final_changes(&operations);
            let admitted = projected_usage(rows, delta, *usage, limits, &changes)?;
            apply_changes(rows, delta, &changes);
            *usage = admitted;
            lsn = seq;
        }
        valid_end = wal.stream_position()?;
    }
}

fn snapshot_path(root: &Path, lsn: u64) -> PathBuf {
    root.join(format!("snapshot-{lsn}.brp"))
}

fn list_snapshots(root: &Path) -> StoreResult<Vec<(u64, PathBuf)>> {
    let mut snapshots = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(lsn) = name.strip_prefix("snapshot-")
            .and_then(|s| s.strip_suffix(".brp"))
            .and_then(|s| s.parse::<u64>().ok()) else { continue };
        if entry.file_type()?.is_file() {
            snapshots.push((lsn, entry.path()));
        }
    }
    snapshots.sort_by_key(|(lsn, _)| *lsn);
    Ok(snapshots)
}

fn read_latest_snapshot(root: &Path, limits: RowStoreLimits) -> StoreResult<(u64, RowMap, RowStoreUsage)> {
    let manifest = checkpoint_manifest::load(root)?;
    let snapshot = match manifest {
        Some(m) => {
            checkpoint_manifest::check_snapshot(root, m)?;
            Some((m.lsn, snapshot_path(root, m.lsn)))
        }
        None => list_snapshots(root)?.pop(), // read-only compatibility with pre-manifest stores
    };
    let Some((lsn, path)) = snapshot else {
        return Ok((0, BTreeMap::new(), RowStoreUsage::default()));
    };
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    if file_len < PAGE_SIZE as u64 || file_len % PAGE_SIZE as u64 != 0 {
        return Err(StoreError::Corruption("snapshot has invalid file length".into()));
    }
    let mut bytes = [0_u8; PAGE_SIZE];
    file.read_exact(&mut bytes)?;
    let head = Page::decode(&bytes)?;
    if head.kind != PageKind::Metadata || head.page_id != 0 || head.lsn != lsn
        || head.payload.len() != META_LENGTH
    {
        return Err(StoreError::Corruption("invalid snapshot metadata page".into()));
    }
    let checkpoint = u64::from_le_bytes(head.payload[..8].try_into().unwrap());
    let count = u64::from_le_bytes(head.payload[8..16].try_into().unwrap());
    let pages = u64::from_le_bytes(head.payload[16..24].try_into().unwrap());
    if checkpoint != lsn || pages.checked_add(1).and_then(|n| n.checked_mul(PAGE_SIZE as u64)) != Some(file_len)
    {
        return Err(StoreError::Corruption("snapshot metadata and file length disagree".into()));
    }
    let mut rows = BTreeMap::new();
    let mut usage = RowStoreUsage::default();
    let mut previous = None::<Vec<u8>>;
    for page_id in 1..=pages {
        file.read_exact(&mut bytes)?;
        let page = Page::decode(&bytes)?;
        if page.kind != PageKind::Data || page.page_id != page_id || page.lsn != lsn {
            return Err(StoreError::Corruption("snapshot data page identity mismatch".into()));
        }
        let mut pos = 0;
        while pos < page.payload.len() {
            if page.payload.len() - pos < ROW_HEADER {
                return Err(StoreError::Corruption("truncated snapshot record".into()));
            }
            let key_len = u16::from_le_bytes(page.payload[pos..pos + 2].try_into().unwrap()) as usize;
            let value_len = u32::from_le_bytes(page.payload[pos + 2..pos + 6].try_into().unwrap()) as usize;
            let size = ROW_HEADER.checked_add(key_len).and_then(|n| n.checked_add(value_len))
                .ok_or_else(|| StoreError::Corruption("snapshot length overflow".into()))?;
            if size > page.payload.len() - pos || key_len == 0 || key_len > MAX_KEY || value_len > MAX_VALUE {
                return Err(StoreError::Corruption("invalid snapshot row length".into()));
            }
            let key = &page.payload[pos + ROW_HEADER..pos + ROW_HEADER + key_len];
            let value = &page.payload[pos + ROW_HEADER + key_len..pos + size];
            if previous.as_ref().is_some_and(|prev| prev.as_slice() >= key) {
                return Err(StoreError::Corruption("snapshot rows are not strictly ordered".into()));
            }
            usage.rows = add_usage(usage.rows,1,"rows",limits.max_rows)?;
            usage.row_bytes = add_usage(usage.row_bytes,row_cost(key,value),"row bytes",limits.max_row_bytes)?;
            limits.check(usage)?;
            previous = Some(key.to_vec());
            rows.insert(key.to_vec(), value.to_vec());
            pos += size;
        }
    }
    if rows.len() as u64 != count {
        return Err(StoreError::Corruption("snapshot row count mismatch".into()));
    }
    if let Some(m) = manifest {
        if m.rows != count || m.data_pages != pages {
            return Err(StoreError::Corruption("checkpoint manifest and snapshot contents disagree".into()));
        }
    }
    Ok((checkpoint, rows, usage))
}

fn write_snapshot(
    file: &mut File,
    lsn: u64,
    rows: &BTreeMap<Vec<u8>, Vec<u8>>,
) -> StoreResult<Vec<(Vec<u8>, RowPointer)>> {
    // First pass computes exact page count without duplicating the complete
    // table into temporary memory. The write path is serialized, so the map
    // cannot change between passes.
    let mut page_count = 0_u64;
    let mut used = 0_usize;
    for (key, value) in rows {
        validate_row(key, value)?;
        let len = ROW_HEADER + key.len() + value.len();
        if len > MAX_PAYLOAD_SIZE {
            return Err(StoreError::InvalidRow("row cannot fit a single physical page"));
        }
        if used + len > MAX_PAYLOAD_SIZE {
            page_count = page_count.checked_add(1).ok_or(StoreError::SequenceExhausted)?;
            used = 0;
        }
        used += len;
    }
    if used != 0 {
        page_count = page_count.checked_add(1).ok_or(StoreError::SequenceExhausted)?;
    }

    let mut meta = Vec::with_capacity(META_LENGTH);
    meta.extend_from_slice(&lsn.to_le_bytes());
    meta.extend_from_slice(&(rows.len() as u64).to_le_bytes());
    meta.extend_from_slice(&page_count.to_le_bytes());
    file.write_all(&Page::new(0, lsn, PageKind::Metadata, meta)?.encode()?)?;

    // Second pass streams physical row pages and retains admitted key/pointer
    // entries for index construction. Admission happened before any file write.
    let mut buffer = Vec::with_capacity(MAX_PAYLOAD_SIZE);
    let mut pointers = Vec::with_capacity(rows.len());
    let mut page_id = 1_u64;
    for (key, value) in rows {
        let len = ROW_HEADER + key.len() + value.len();
        if buffer.len() + len > MAX_PAYLOAD_SIZE {
            file.write_all(&Page::new(page_id, lsn, PageKind::Data, std::mem::take(&mut buffer))?.encode()?)?;
            page_id = page_id.checked_add(1).ok_or(StoreError::SequenceExhausted)?;
            buffer = Vec::with_capacity(MAX_PAYLOAD_SIZE);
        }
        pointers.push((key.clone(), RowPointer {
            page_id,
            offset: u16::try_from(buffer.len()).expect("payload offset fits u16"),
        }));
        buffer.extend_from_slice(&(key.len() as u16).to_le_bytes());
        buffer.extend_from_slice(&(value.len() as u32).to_le_bytes());
        buffer.extend_from_slice(key);
        buffer.extend_from_slice(value);
    }
    if !buffer.is_empty() {
        file.write_all(&Page::new(page_id, lsn, PageKind::Data, buffer)?.encode()?)?;
        page_id = page_id.checked_add(1).ok_or(StoreError::SequenceExhausted)?;
    }
    if page_id != page_count.checked_add(1).ok_or(StoreError::SequenceExhausted)? {
        return Err(StoreError::Corruption("checkpoint page count changed during write".into()));
    }
    Ok(pointers)
}


fn read_indexed_record(
    cache: &mut PageCache,
    expected_key: &[u8],
    pointer: RowPointer,
) -> StoreResult<Vec<u8>> {
    let page = cache.read(pointer.page_id)?;
    if page.kind != PageKind::Data {
        return Err(StoreError::Corruption("index points to non-data checkpoint page".into()));
    }
    let offset = pointer.offset as usize;
    if offset > page.payload.len() || page.payload.len() - offset < ROW_HEADER {
        return Err(StoreError::Corruption("index points outside a row header".into()));
    }
    let key_len = u16::from_le_bytes(page.payload[offset..offset+2].try_into().unwrap()) as usize;
    let value_len = u32::from_le_bytes(page.payload[offset+2..offset+6].try_into().unwrap()) as usize;
    let end = ROW_HEADER.checked_add(key_len).and_then(|n| n.checked_add(value_len))
        .and_then(|n| n.checked_add(offset))
        .ok_or_else(|| StoreError::Corruption("indexed row length overflow".into()))?;
    if key_len == 0 || key_len > MAX_KEY || value_len > MAX_VALUE || end > page.payload.len()
        || &page.payload[offset+ROW_HEADER..offset+ROW_HEADER+key_len] != expected_key
    {
        return Err(StoreError::Corruption("index row pointer or key mismatched".into()));
    }
    Ok(page.payload[offset+ROW_HEADER+key_len..end].to_vec())
}

fn prune_snapshots(root: &Path, keep: u64) -> StoreResult<()> {
    for (lsn, path) in list_snapshots(root)? {
        if lsn < keep {
            fs::remove_file(path)?;
            let index_path = root.join(format!("index-{lsn}.bri"));
            if index_path.exists() { fs::remove_file(index_path)?; }
        }
    }
    sync_directory(root)?;
    Ok(())
}

#[cfg(unix)]
fn sync_directory(root: &Path) -> std::io::Result<()> {
    File::open(root)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_root: &Path) -> std::io::Result<()> {
    // Windows needs a separately tested directory-replacement durability layer.
    Ok(())
}

#[cfg(test)]
mod resource_tests {
    use super::*;
    #[test]
    fn map_accounting_overflow_refuses_instead_of_wrapping() {
        for resource in ["rows","row bytes","delta keys","delta bytes"] {
            assert!(matches!(add_usage(usize::MAX,1,resource,usize::MAX),
                Err(ResourceError::RowStoreLimit { resource:actual,.. }) if actual==resource));
        }
    }
}
