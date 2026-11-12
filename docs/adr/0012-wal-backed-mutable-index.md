# ADR 0012 — WAL-backed mutable logical index over immutable physical B+tree

**Product:** Bradian Database Systems · SRS v1.1  
**Status:** Source implementation committed on `main`; pending manually executed build and recovery tests.  
**Depends on:** [ADR 0008 — WAL and checkpoint](0008-native-row-wal-checkpoint.md), [ADR 0010 — checkpoint B+tree](0010-checkpoint-btree.md), [ADR 0011 — generation-scoped cache](0011-generation-scoped-buffer-pool.md).

## Motivation

G5 introduced a real multi-level 4 KiB on-disk B+tree over a complete, immutable row checkpoint. G6 added a bounded page cache, but any new WAL mutation forced indexed point and range queries to bypass the disk index entirely and use the full in-memory row map. That erased the read benefit of the index between checkpoints and prevented incremental updates from contributing to the indexed query plan.

In-place mutable index pages require a durable page allocator, page-image or physiological redo records, WAL-before-data enforcement, torn-page recovery, page latching, and split/merge recovery. Introducing partially implemented in-place writes would undermine Bradian's source-of-truth and crash consistency rules.

## Decision

Keep each published `index-<checkpoint_lsn>.bri` B+tree **immutable**. Introduce a **mutable logical index overlay** of the keys changed since that checkpoint:

```text
                    Committed WAL (authority)
                            |
                 +----------+------------+
                 |                       |
       Full committed row map     Mutable delta index
        (correctness oracle)      key -> value | tombstone
                                         |
                    +--------------------+----------------+
                    |                                     |
              Point lookup                          Range lookup
          delta first, disk fallback         disk B+tree + sorted delta
                    |                                     |
                    +---------------+---------------------+
                                    |
                    Physical snapshot BRPG rows
                         (unchanged values)
```

The overlay is an ordered `BTreeMap<Vec<u8>, Option<Vec<u8>>>`, using `Some(value)` for an inserted/replaced key and `None` for a committed deletion tombstone. It is **not a second independent durable database**; its contents are always reconstructed from the accepted WAL frames after the latest published checkpoint.

## Commit and recovery protocol

1. For a single put/delete or atomic `commit_batch`, Bradian validates input and serializes a complete framed WAL record.
2. The log record is written and `sync_all()` succeeds **before** its mutations appear in memory.
3. Under the existing exclusive `parking_lot::Mutex`, successful mutations update both the full committed row map and the delta index. Failed writes do not change either structure; failed I/O poisons the store.
4. During restart, `RowStore::open()` loads the published BRPG snapshot, then replays complete, CRC-verified and sequentially numbered WAL records newer than the checkpoint. It applies the same mutations to both maps. A malformed record fails closed; only a provably incomplete final frame is truncated.
5. A completed checkpoint publishes a new authoritative physical snapshot and corresponding derived B+tree, then truncates and syncs redundant WAL bytes, resets the in-memory delta, and invalidates old index/data-page caches.

This protocol preserves the preexisting single-writer and WAL atomicity guarantees. One accepted batch commit has one WAL frame and one LSN; all its mutations appear in the new overlay together under the process mutex.

## Query semantics

**Point lookup:** When a key exists in the delta, return its value or tombstone. Otherwise, traverse the published disk B+tree and read the corresponding checksummed physical row page through G6's generation-scoped cache. If the checkpoint has no published index, the authoritative committed row map provides a safe compatibility fallback.

**Range lookup:** Traverse the immutable B+tree over the requested half-open interval `[start, end)`, resolve physical checkpoint rows, and merge the ordered delta entries for the same interval. A `Some(value)` entry replaces or adds a key; a `None` tombstone removes it. The result is ordered, unique, and includes all commits accepted before the query acquired the store mutex.

Both read paths compare their final result with the full committed row map as a defensive integrity invariant. A mismatch is a corruption error, not an empty result. The disk index is never expected to contain post-checkpoint mutations.

## Correctness and operational bounds

- **Durability:** The overlay itself is transient; WAL replay is its durable change source. It must never include an unacknowledged mutation.
- **Atomic batches:** Updates of many keys are represented by one WAL frame, one LSN, and one critical section.
- **Delete semantics:** A deleted key remains represented by a tombstone even if it existed in the disk checkpoint.
- **Ordering:** Key comparison is bytewise. SQL collation and typed numeric sort order remain separate future contracts.
- **Memory:** Distinct modified keys accumulate until the next checkpoint, and the full committed map still occupies RAM. Checkpoint scheduling, backpressure and size caps are needed for production scale.
- **Index health:** The immutable B+tree and BRPG pages retain strict corruption reporting; an altered key resolved directly from the WAL overlay does not need to touch unaffected disk pages.
- **Concurrency:** The current mutex provides serialized single-node operations, not MVCC, row-level locks or snapshot isolation.
- **Physical mutability:** Index nodes are still reconstructed at checkpoints. In-place page splitting, merging, dirty-page eviction and WAL-protected page updates remain **unimplemented**.

## Test evidence in source

`crates/bradian-relational/tests/btree_index.rs` now includes delta-over-checkpoint point lookup, range merge, tombstone preservation, new-key visibility, checkpoint cache reuse, restart reconstruction, post-checkpoint delta clearing and atomic WAL batch behavior. Historical G3/G5/G6 tests continue to cover WAL integrity, immutable disk pages, corruption, recovery and buffer-pool behavior.

Automatic CI remains disabled by owner instruction. The source and regression tests are committed, but manual Cargo checks and platform fault injection must pass before accepting this implementation gate.
