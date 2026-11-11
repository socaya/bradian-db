# ADR 0003 — Durable WAL, atomic manifests, delta memtable and snapshots

**Status:** Accepted · 2026-11-11
**Spec sections:** §3.1, §3.5, §3.6, requirements STO-002/004/005/006

## Context

The analytical store must commit atomically, survive crashes without acknowledged logical
loss, and let concurrent readers see a consistent snapshot while compaction and ingestion
proceed in the background.

## Decisions

### WAL

- One append-only file per partition: `analytics_root/datasets/<d>/partitions/<p>/wal.log`.
- Records are length-prefixed frames carrying a `WalRecord` bincode payload and a CRC32 over
  the payload. Record layout:
  ```
  u32 payload_len · u32 payload_crc32 · u8[payload_len]
  ```
- `WalRecord` variants:
  - `Begin { lsn, started_at_ms }`
  - `Upsert { lsn, key: PrimaryKey, source_version: u64, row: SerializedRow }`
  - `Delete { lsn, key: PrimaryKey, source_version: u64 }`
  - `Checkpoint { lsn, visible_generation: u64, watermark_ms: i64 }`
  - `Commit { lsn, tx_id: u64 }`
- The first `Begin`/`Upsert`/`Delete`/`Commit` of each process run uses `lsn=1`; subsequent
  records increment by one. On open the WAL is scanned in order; a record that fails CRC or
  truncates the file **terminates replay** without discarding earlier accepted records. No
  structural edit to WAL is permitted during replay.

### Manifest

- `analytics_root/datasets/<d>/partitions/<p>/manifest.json` — a small JSON document with:
  - `generation: u64`
  - `visible_segments: [{ segment_id, path, size_bytes, row_count, watermark_ms, data_revision }]`
  - `tombstones: [{ delete_vector_path, size_bytes }]` (for STO-006, reserved field)
  - `wal_truncation_lsn: u64` — WAL replay below this LSN can be discarded after a successful
    checkpoint.
  - `schema_fingerprint`, `metadata_revision`.
- Publication protocol:
  1. Write `manifest.json.tmp` with the new generation and its CRC32 trailer.
  2. `fsync` the tmp file.
  3. `rename` tmp → `manifest.json`.
  4. `fsync` the parent directory.
- A `ManifestCorrupt` error reverts to the last known good generation (Linux keeps a copy at
  `manifest.json.bak`). If both copies are corrupt, replay rebuilds the partition from WAL +
  segment files.

### Delta (memtable)

- A `Delta` holds row-level upserts and deletes that have not yet been compacted into a
  segment. Keys are `PrimaryKey` (dataset-defined composite); values keep `(source_version,
  row)` so later arrivals with a higher version supersede earlier ones. Deletes are stored
  as sentinel entries with a tombstone marker, so `Delete→Upsert→Delete` with monotonic
  versions converges to the final state.
- On query, the delta is merged with visible segments via snapshot-consistent read: scan
  returns delta rows with highest version per key, union with segment rows whose key is
  not superseded and not tombstoned. Deletes act as filters at scan time.

### Snapshots

- A `Snapshot` is a cheap handle containing `(generation, watermark_ms)`. Readers hold a
  snapshot for the duration of a query. Compaction writes new segments and bumps the
  manifest generation; the old generation's segments remain on disk until all snapshots
  referencing them drop. Reference counts live in-memory in the `Store` and are not
  persisted — process restart drops all snapshots and compaction GC is safe to proceed.

### Compaction

- v1 scope: flush-delta compaction only. When the delta exceeds a size budget (rows or
  bytes), writer emits a new `.derc` segment containing the merged state and publishes a
  new manifest that supersedes the delta's contributions. Multi-segment merging is
  reserved for a later gate.

## Consequences

- Durability is anchored on file-level `fsync` and rename atomicity (POSIX guarantee on
  the target filesystems: ext4, xfs, apfs). The write-and-rename sequence is well-studied
  and matches the pattern used by SQLite and LMDB.
- The JSON manifest is readable by operators at rest. Binary manifests are rejected here
  because operability trumps a negligible-size win for a < 1 MB manifest.
- The delta is a Rust `BTreeMap` for v1. A later gate may swap in a skiplist or LSM
  substructure once we have real workload traces.
