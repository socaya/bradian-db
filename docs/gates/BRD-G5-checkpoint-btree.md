# BRD-G5 — Native physical B+tree checkpoint index

**Master specification:** Bradian Database Systems SRS v1.1  
**Architecture decision:** [ADR 0010](../adr/0010-checkpoint-btree.md)  
**Status:** Complete code slice and integration tests committed to `main`; not qualified as a production mutable B+tree.

## Implemented functions

The `bradian-relational::btree` module now creates and reads a genuine multi-level B+tree held in portable 4 KiB physical pages. It contains sorted linked leaf pages and internal separator pages with real child pointers. The tree is built from the authoritative row checkpoint and stores row-page addresses, not duplicate row values.

`RowStore::checkpoint()` publishes immutable BRPG snapshot pages, then writes and publishes the matching B+tree, and only then truncates the WAL. This preserves the authoritative data across crashes or interrupted index creation. `RowStore::indexed_get()` and `indexed_range()` traverse the disk index when no newer WAL changes exist; otherwise they use the committed memory map. Typed table point queries use this path transparently.

## Committed integration tests

- [x] Empty tree and single-leaf point lookups.
- [x] Multi-level tree with thousands of keys, depth traversal and range scans.
- [x] Duplicate and out-of-order build inputs rejected.
- [x] Invalid row pointers, metadata and CRC damage rejected.
- [x] On-disk indexed lookup against published row checkpoint pages.
- [x] Indexed ranges crossing linked-leaf page boundaries.
- [x] Reads after newer WAL upserts and deletes never return stale index values.
- [x] Recovery and index access after process restart.
- [x] Corrupt index never produces a false missing-row result.
- [x] Old index files pruned only after new checkpoint publication.
- [x] Typed table key lookup via indexed pages and corruption propagation.

These refer to tests committed in source, **not tests already executed**.

## What this milestone does not claim

No in-place B+tree updates, page splitting during active transactions, free-page reuse, distributed secondary indexing, MVCC snapshots, B+tree latching, SQL collation, write-back buffer cache, non-blocking online checkpoint, index encryption, full index rebuild tool or disk-first query engine has been completed. Checkpoints still hold a process-local lock and the working row map is in RAM. The index is a durable accelerator, not a separate source of truth.

## Required acceptance verification

- [ ] `cargo check --workspace --locked`
- [ ] `cargo test -p bradian-relational --locked`
- [ ] `cargo test --workspace --locked`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] Linux/macOS/Windows page compatibility and crash-injection qualification
- [ ] Disk-full, out-of-space, atomic-rename and partial-write fault injection
- [ ] B+tree root/leaf damage handling and controlled rebuild procedure
- [ ] Performance benchmarks for 1M+ rows, multiple page sizes and high read concurrency

Manual checks are required since the user has disabled automatic CI.
