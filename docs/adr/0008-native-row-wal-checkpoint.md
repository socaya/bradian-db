# ADR 0008 — Native row WAL, page checkpoint, and restart recovery

**Status:** Implemented as a single-writer, individually atomic row-store slice; full production qualification remains open.  
**Product:** Bradian Database Systems · SRS v1.1  
**Companion:** [ADR 0007 — Portable physical page](0007-portable-physical-page.md)

## Problem and architectural boundary

Bradian's first generic relational catalog and physical page codec were in-memory structures and binary primitives. They did not persist user rows or provide recovery after a process restart. This decision introduces a functional, non-DHIS2 persistent key/value row store while retaining the existing analytical `.derc` format and the original DAP conventions: append-only checksummed log records, per-record synchronization, deterministic maps, explicit checkpoints, strict decoding and actual file-based tests.

This implements complete single-key persistence and an **atomic local multi-key batch-commit primitive** rather than a pretend full SQL DBMS. It provides persistent inserts, replacements, deletes, ordered scans, checksummed transaction frames, checkpoints and recovery. It does not implement multi-row SQL transactions, MVCC, a B+tree, a persistent relational catalog, query planning, SQL constraints, cluster replication, file encryption or multi-process file locking.

## Data and log files

For a caller-supplied store directory, the implementation creates:

- `rows.wal` — append-only row mutation log, synchronized on each accepted write.
- `snapshot-<lsn>.tmp` — unpublished checkpoint construction file; ignored during restart.
- `snapshot-<lsn>.brp` — published, immutable physical-page snapshot. Only the highest numbered published snapshot is selected during restart; corrupted published snapshots fail closed rather than silently falling back.

The WAL is a series of frames, each containing a 12-byte envelope and a checksummed body:

| Offset | Width | Field |
|---|---:|---|
| 0 | 4 | `BRWL` magic |
| 4 | 4 | Body length, little-endian u32 |
| 8 | 4 | CRC32 of the complete body |
| 12 | 8 | Monotonic log sequence number (LSN) |
| 20 | 1 | Operation code: 1 upsert, 2 delete |
| 21 | 2 | Key length, little-endian u16 |
| 23 | 4 | Value length, little-endian u32 |
| 27+ | Variable | Raw key bytes followed by raw value bytes |

The current inline layout supports keys of **1–512 bytes** and values of **0–3,546 bytes**; it deliberately rejects oversize records before modifying the log. Future overflow pages and typed row layouts will extend the record model through versioned formats instead of silently corrupting this one. Legacy WAL operation codes **1** (upsert) and **2** (delete) each represent one individually atomic mutation. A new WAL opcode **3** represents an entire atomically committed batch: `u64 LSN · u8 opcode=3 · u32 mutation_count · repeated[u8 sub-operation · u16 key length · u32 value length · key · value]`. Batch frames are bounded to 256 operations and 1 MiB of serialized WAL body. One frame receives one LSN and one `sync_all`. Existing single-operation frames remain readable. Strict decoding validates the complete batch before applying a single change. The sequential LSN is an ordering marker, not a distributed consensus timestamp.

## Commit ordering

A write acquires the store's in-process mutex, validates its key and value, assigns the next LSN, appends a complete frame and calls `File::sync_all()`. Only after synchronization succeeds does the implementation publish the value change into the in-memory ordered map and return its LSN. A failed write or synchronization marks that open instance poisoned to prevent further writes until reopening and recovering the log.

A synchronized delete of a nonexistent key is still recorded as an idempotent delete with a new LSN. The `commit_batch(expected_lsn, operations)` API additionally supports optional optimistic global-LSN verification. When the observed LSN differs, the entire batch fails **without appending any WAL bytes**. The current batch API is a locally serialized commit primitive; it does not provide arbitrary SQL statements, snapshot isolation, row-level locking, MVCC or distributed two-phase commit. This guarantees that subsequent recovery and ordering do not treat no-op deletions differently from other acknowledged operations.

## Page checkpoint protocol

A checkpoint freezes the in-memory view by holding the same mutex as writers, composes sorted row records into the existing BRPG physical-page envelope, and writes:

1. Page 0: page kind `Metadata` (4), checkpoint LSN, row count, data-page count.
2. Pages 1..N: page kind `Data` (1), compact length-prefixed records, with page IDs and the same checkpoint LSN.

Each page carries the physical-page CRC, and its payload and tail are validated when loaded. The checkpoint temp file is synchronized, renamed to an immutable published `.brp` filename, and the directory is synchronized on Unix. **Only then** is `rows.wal` truncated and synchronized. Old published snapshots are removed after this boundary on a best-effort basis. This sequencing avoids acknowledging a snapshot that depends on already-discarded log records.

Publishing a snapshot before truncating the old log is deliberate. If the process crashes during that interval, recovery loads the complete checkpoint and skips WAL records already represented by its LSN.

## Restart recovery and corruption rules

On open, the store loads the newest published snapshot (if present) and verifies the entire file length, all page CRCs, page order and types, LSN consistency, record lengths, global key ordering and expected row count. Snapshot corruption is a hard error. Recovery then sequentially validates the WAL frame magic, bounded frame length, CRC, operation, key and value lengths, and LSN continuity.

A **truncated final WAL header or body** is discarded back to the last completely verified frame. Corrupt magic, a CRC mismatch, an illegal operation, or an inconsistent sequence stops recovery with a corruption error rather than silently abandoning potentially acknowledged data. Fully recovered rows become available only after the replay completes.

## Security and environmental constraints

- A CRC protects against accidental corruption; it is not authenticated encryption or protection against an adversary able to rewrite data and checksums.
- Bradian currently relies on the caller to ensure **exclusive access to the store directory across processes**. The mutex only serializes threads inside one RowStore instance. Simultaneous RowStore instances on the same directory are unsafe until OS-level file locking is implemented and tested.
- Unix directory synchronization is included; equivalent atomic publication and power-loss guarantees on Windows and other filesystems remain subject to separate platform qualification.
- Long-running stores require periodic checkpoints; a WAL retained indefinitely grows without bound.
- The representation is an inline key/value page store, not a complete SQL row representation or disk-resident B+tree. The in-memory BTreeMap is rebuilt from disk on restart.
- Neither this implementation nor an in-memory mutex guarantees isolation of multi-statement transactions.

## Verification evidence

Tests in `crates/bradian-relational/tests/row_store_durability.rs` additionally cover atomic multi-key WAL batches, optional LSN compare-and-swap conflict rejection, torn final batch frames, and malformed batch bodies with a recomputed CRC. Other tests cover committed replay across restarts, updates and deletes, multiple physical checkpoint pages, checkpoint idempotence, post-checkpoint replay, truncated header/body repair, CRC corruption failure, published snapshot corruption failure, unpublished temp snapshot rejection, duplicate WAL replay through checkpoint boundaries, invalid input rejection, ordered binary keys and repeated checkpoints.

**No automatic CI runs are required under the current owner instruction.** Run the following manually before accepting the gate:

```sh
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Do not mark production row storage, WAL durability, transaction isolation, encrypted storage or cross-platform crash guarantees as accepted until the corresponding remaining engineering gates pass.
