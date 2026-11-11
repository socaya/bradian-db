//! `Partition` — the composition point for WAL + delta + manifest + snapshot references.
//!
//! Spec: §3.1, §3.5, §3.6. ADR 0003 for durability design.
//!
//! The partition is single-writer multiple-reader. Writes acquire a per-partition lock
//! around the WAL append + delta mutation. Reads take a snapshot and proceed without
//! blocking writes. Compaction holds the write lock briefly to swap in a new manifest.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dap_core::{DapError, DapResult, Fingerprint, Schema};
use parking_lot::{Mutex, RwLock};

use crate::segment::{SegmentReader, SegmentWriter, SegmentWriterOptions};

use super::delta::{Delta, PrimaryKey, Row};
use super::manifest::{Manifest, ManifestSegment};
use super::snapshot::{Snapshot, SnapshotRef, SnapshotRefs};
use super::wal::{WalReader, WalRecord, WalWriter};

#[derive(Clone, Debug)]
pub struct PartitionOptions {
    pub dataset_id: String,
    pub partition_id: String,
    pub schema: Schema,
    /// Columns composing the primary key (by schema order).
    pub primary_key_columns: Vec<String>,
    /// Flush the delta when it exceeds this many rows.
    pub flush_rows: usize,
    /// Flush the delta when it exceeds this many bytes (approx).
    pub flush_bytes: usize,
}

#[derive(Debug)]
pub struct Partition {
    dir: PathBuf,
    options: PartitionOptions,
    primary_key_indexes: Vec<usize>,
    schema_fingerprint: Fingerprint,

    inner: RwLock<Inner>,
    wal: Mutex<WalWriter>,
    snapshot_refs: SnapshotRef,
}

#[derive(Debug)]
struct Inner {
    manifest: Manifest,
    delta: Delta,
    delta_version: u64,
    next_lsn: u64,
    next_data_revision: u64,
    next_segment_id: u64,
}

impl Partition {
    pub fn open(root: &Path, options: PartitionOptions) -> DapResult<Arc<Self>> {
        let dir = partition_dir(root, &options.dataset_id, &options.partition_id);
        std::fs::create_dir_all(&dir)?;
        std::fs::create_dir_all(dir.join("segments"))?;

        let primary_key_indexes = options
            .primary_key_columns
            .iter()
            .map(|n| {
                options.schema.index_of(n).map_err(|_| {
                    DapError::schema_mismatch(format!("pk column '{n}' not in schema"))
                })
            })
            .collect::<DapResult<Vec<_>>>()?;

        let schema_fingerprint = Fingerprint::of(&serde_json::to_vec(&options.schema).unwrap());

        let manifest = Manifest::load(&dir)?
            .unwrap_or_else(|| Manifest::new(schema_fingerprint));
        if manifest.schema_fingerprint != schema_fingerprint {
            return Err(DapError::schema_mismatch(
                "stored manifest schema fingerprint does not match open options",
            ));
        }

        // Replay WAL records that post-date the last manifest checkpoint.
        let wal_path = dir.join("wal.log");
        let replay = WalReader::open(&wal_path).replay()?;
        let mut delta = Delta::new();
        let mut next_lsn = manifest.wal_truncation_lsn + 1;
        for rec in &replay {
            match rec {
                WalRecord::Upsert {
                    lsn,
                    key,
                    source_version,
                    row,
                } => {
                    delta.upsert(key.clone(), row.clone(), *source_version, *lsn);
                    next_lsn = next_lsn.max(*lsn + 1);
                }
                WalRecord::Delete {
                    lsn,
                    key,
                    source_version,
                } => {
                    delta.delete(key.clone(), *source_version, *lsn);
                    next_lsn = next_lsn.max(*lsn + 1);
                }
                WalRecord::Begin { lsn, .. }
                | WalRecord::Commit { lsn, .. }
                | WalRecord::Checkpoint { lsn, .. } => {
                    next_lsn = next_lsn.max(*lsn + 1);
                }
            }
        }

        let next_data_revision = manifest
            .visible_segments
            .iter()
            .map(|s| s.data_revision)
            .max()
            .unwrap_or(0)
            + 1;
        let next_segment_id = manifest.visible_segments.len() as u64 + 1;

        let wal = WalWriter::open(&wal_path)?;

        Ok(Arc::new(Self {
            dir,
            options,
            primary_key_indexes,
            schema_fingerprint,
            inner: RwLock::new(Inner {
                manifest,
                delta,
                delta_version: 0,
                next_lsn,
                next_data_revision,
                next_segment_id,
            }),
            wal: Mutex::new(wal),
            snapshot_refs: Arc::new(Mutex::new(SnapshotRefs::default())),
        }))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn schema(&self) -> &Schema {
        &self.options.schema
    }

    pub fn schema_fingerprint(&self) -> Fingerprint {
        self.schema_fingerprint
    }

    pub fn snapshot(&self) -> Snapshot {
        let inner = self.inner.read();
        Snapshot::new(
            inner.manifest.generation,
            inner.delta_version,
            self.snapshot_refs.clone(),
        )
    }

    pub fn visible_segments(&self) -> Vec<ManifestSegment> {
        self.inner.read().manifest.visible_segments.clone()
    }

    /// Append a full row.
    pub fn upsert_row(&self, row: Vec<dap_core::Value>, source_version: u64) -> DapResult<()> {
        if row.len() != self.options.schema.len() {
            return Err(DapError::schema_mismatch("row width != schema width"));
        }
        let key = self.extract_key(&row)?;
        let mut wal = self.wal.lock();
        let mut inner = self.inner.write();
        let lsn = inner.next_lsn;
        inner.next_lsn += 1;
        wal.append_and_sync(&WalRecord::Upsert {
            lsn,
            key: key.clone(),
            source_version,
            row: Row(row.clone()),
        })?;
        if inner.delta.upsert(key, Row(row), source_version, lsn) {
            inner.delta_version += 1;
        }
        self.maybe_flush(&mut inner, &mut wal)?;
        Ok(())
    }

    pub fn delete_row(
        &self,
        key_values: Vec<dap_core::Value>,
        source_version: u64,
    ) -> DapResult<()> {
        let key = PrimaryKey(key_values);
        let mut wal = self.wal.lock();
        let mut inner = self.inner.write();
        let lsn = inner.next_lsn;
        inner.next_lsn += 1;
        wal.append_and_sync(&WalRecord::Delete {
            lsn,
            key: key.clone(),
            source_version,
        })?;
        if inner.delta.delete(key, source_version, lsn) {
            inner.delta_version += 1;
        }
        self.maybe_flush(&mut inner, &mut wal)?;
        Ok(())
    }

    /// Force a flush of the current delta to a new segment. Used by tests, by admin operations
    /// and by ingestion checkpoints.
    pub fn flush(&self) -> DapResult<Option<ManifestSegment>> {
        let mut wal = self.wal.lock();
        let mut inner = self.inner.write();
        self.flush_locked(&mut inner, &mut wal)
    }

    /// Read the live state for a key. Checks the delta first, then any visible segment where
    /// the schema primary-key columns match. Returns `None` if the row is tombstoned or absent.
    pub fn get(&self, key: &PrimaryKey) -> DapResult<Option<Row>> {
        let inner = self.inner.read();
        if let Some(entry) = inner.delta.get(key) {
            if entry.is_tombstone() {
                return Ok(None);
            }
            return Ok(entry.row.clone());
        }
        // Fall through: scan visible segments. Limited to v1 — this is the only point in the
        // codebase that currently reads segments by key. Hot queries go through the runtime
        // scan path once that lands.
        for seg in &inner.manifest.visible_segments {
            if let Some(row) = self.segment_lookup(seg, key)? {
                return Ok(Some(row));
            }
        }
        Ok(None)
    }

    /// Materialise the logical state of the partition as a vector of rows, merging visible
    /// segments with the in-memory delta. Delta upserts supersede segment rows with the same
    /// primary key; delta tombstones remove segment rows.
    ///
    /// Snapshot-consistent: the method reads the manifest + delta under the read lock, then
    /// opens segment files without the lock held. Compaction cannot delete pages referenced
    /// here because the caller retains a `Snapshot` handle for the duration of the scan.
    pub fn snapshot_rows(&self) -> DapResult<Vec<Row>> {
        let inner = self.inner.read();
        let segments = inner.manifest.visible_segments.clone();
        let delta_entries: Vec<_> = inner
            .delta
            .iter()
            .map(|(k, e)| (k.clone(), e.clone()))
            .collect();
        drop(inner);

        let mut keyed: ahash::AHashMap<PrimaryKey, Row> = ahash::AHashMap::default();
        for seg in &segments {
            let mut reader = SegmentReader::open(&seg.path)?;
            let n = reader.footer().row_count as usize;
            if n == 0 {
                continue;
            }
            let mut columns = Vec::with_capacity(self.options.schema.len());
            for c in &self.options.schema.columns {
                columns.push(reader.read_column(&c.name)?);
            }
            for i in 0..n {
                let mut row = Vec::with_capacity(self.options.schema.len());
                for col in &columns {
                    row.push(col.value_at(i)?);
                }
                let key = self.extract_key(&row)?;
                keyed.insert(key, Row(row));
            }
        }

        for (key, entry) in delta_entries {
            match entry.row {
                None => {
                    keyed.remove(&key);
                }
                Some(r) => {
                    keyed.insert(key, r);
                }
            }
        }

        Ok(keyed.into_values().collect())
    }

    fn segment_lookup(&self, seg: &ManifestSegment, key: &PrimaryKey) -> DapResult<Option<Row>> {
        let mut reader = SegmentReader::open(&seg.path)?;
        let n = reader.footer().row_count as usize;
        let mut columns = Vec::with_capacity(self.options.schema.len());
        for c in &self.options.schema.columns {
            columns.push(reader.read_column(&c.name)?);
        }
        for i in 0..n {
            let mut row = Vec::with_capacity(self.options.schema.len());
            for col in &columns {
                row.push(col.value_at(i)?);
            }
            let this_key = self.extract_key(&row)?;
            if &this_key == key {
                return Ok(Some(Row(row)));
            }
        }
        Ok(None)
    }

    fn extract_key(&self, row: &[dap_core::Value]) -> DapResult<PrimaryKey> {
        let mut key = Vec::with_capacity(self.primary_key_indexes.len());
        for idx in &self.primary_key_indexes {
            key.push(row[*idx].clone());
        }
        Ok(PrimaryKey(key))
    }

    fn maybe_flush(&self, inner: &mut Inner, wal: &mut WalWriter) -> DapResult<()> {
        let rows = inner.delta.len();
        let bytes = inner.delta.bytes_estimate();
        if rows >= self.options.flush_rows || bytes >= self.options.flush_bytes {
            self.flush_locked(inner, wal)?;
        }
        Ok(())
    }

    fn flush_locked(
        &self,
        inner: &mut Inner,
        wal: &mut WalWriter,
    ) -> DapResult<Option<ManifestSegment>> {
        if inner.delta.is_empty() {
            return Ok(None);
        }
        // Freeze the delta.
        let frozen = std::mem::take(&mut inner.delta);
        let watermark = frozen
            .iter()
            .map(|(_, e)| e.lsn)
            .max()
            .unwrap_or(0) as i64;
        let next_lsn_before_flush = inner.next_lsn;

        // Build a segment by writing each non-tombstone row through SegmentWriter.
        let segment_id = format!("seg-{:08}.derc", inner.next_segment_id);
        inner.next_segment_id += 1;
        let path = self.dir.join("segments").join(&segment_id);

        let mut writer_opts = SegmentWriterOptions {
            dataset_id: self.options.dataset_id.clone(),
            partition_id: self.options.partition_id.clone(),
            segment_id: segment_id.clone(),
            compress: true,
            dict_cardinality_threshold: 0.5,
            watermark_ms: watermark,
            data_revision: inner.next_data_revision,
            metadata_revision: inner.manifest.metadata_revision,
        };
        let data_rev = writer_opts.data_revision;
        inner.next_data_revision += 1;

        let mut writer = SegmentWriter::new(self.options.schema.clone(), writer_opts.clone());
        let mut row_count = 0u64;
        for (_key, entry) in frozen.iter() {
            if let Some(row) = &entry.row {
                writer.append_row(&row.0)?;
                row_count += 1;
            }
        }
        if row_count == 0 {
            // Entirely tombstones: nothing to publish. Still bump the WAL truncation LSN.
            writer_opts.data_revision = data_rev; // sink
            let generation = inner.manifest.generation + 1;
            inner.manifest.generation = generation;
            inner.manifest.wal_truncation_lsn = next_lsn_before_flush.saturating_sub(1);
            inner.manifest.save(&self.dir)?;
            wal.append_and_sync(&WalRecord::Checkpoint {
                lsn: inner.next_lsn,
                visible_generation: generation,
                watermark_ms: watermark,
            })?;
            inner.next_lsn += 1;
            wal.truncate()?;
            inner.delta_version += 1;
            return Ok(None);
        }
        let written = writer.finish_to_path(&path)?;

        let seg_meta = ManifestSegment {
            segment_id: segment_id.clone(),
            path: written.path,
            size_bytes: written.size_bytes,
            row_count,
            watermark_ms: watermark,
            data_revision: data_rev,
        };

        // Publish new manifest generation.
        let generation = inner.manifest.generation + 1;
        inner.manifest.generation = generation;
        inner.manifest.visible_segments.push(seg_meta.clone());
        inner.manifest.wal_truncation_lsn = next_lsn_before_flush.saturating_sub(1);
        inner.manifest.save(&self.dir)?;

        // WAL checkpoint + truncate.
        wal.append_and_sync(&WalRecord::Checkpoint {
            lsn: inner.next_lsn,
            visible_generation: generation,
            watermark_ms: watermark,
        })?;
        inner.next_lsn += 1;
        wal.truncate()?;

        // Advance delta version so snapshots know pre-flush state is no longer available
        // through the delta.
        inner.delta_version += 1;

        Ok(Some(seg_meta))
    }
}

fn partition_dir(root: &Path, dataset: &str, partition: &str) -> PathBuf {
    root.join("datasets").join(dataset).join("partitions").join(partition)
}
