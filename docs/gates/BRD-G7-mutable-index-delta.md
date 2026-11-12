# BRD-G7 — Incrementally mutable WAL-backed index overlay

**SRS:** Bradian Master Architecture and SRS v1.1  
**Decision:** [ADR 0012](../adr/0012-wal-backed-mutable-index.md)  
**Implementation:** `crates/bradian-relational/src/row_store.rs`  
**Status:** Functional code and regression tests committed to `main`; no automatic CI, manual test evidence pending.

## Scope delivered

The previous immutable physical B+tree is now integrated with a **mutable logical index overlay** containing all committed key changes since the latest published checkpoint. Every successful `put`, `delete` and atomic `commit_batch` updates the ordered overlay *after* WAL synchronization. Restart recovery reconstructs it by applying validated WAL frames to the published checkpoint.

`indexed_get` resolves delta keys first, then unchanged keys through the G5 physical B+tree and G6 page caches. `indexed_range` performs a sorted merge of physical checkpoint entries and post-checkpoint delta entries, including deletions as tombstones. Both paths compare their results against the authoritative committed row map to detect divergence.

Checkpoint publication resets the overlay only after its new snapshot and B+tree are safely published, and invalidates the old physical page caches. This design avoids storing uncommitted modifications in index pages and preserves the existing CRC/LSN recovery contract.

## Source tests

- [x] Post-checkpoint index overlay point updates, inserts and deletes.
- [x] Repeat queries continue using the published physical index and cache.
- [x] Sorted merged range with replacements, inserted keys and repeated tombstones.
- [x] WAL replay reconstructs delta after restart.
- [x] Checkpoint clears delta and subsequent restart loads new physical generation.
- [x] One synchronized multi-key batch updates overlay atomically.
- [x] Pre-existing G3/G5/G6 WAL and index corruption tests remain present.

These entries mean **test cases are committed**, not that they were executed or passed.

## Manual verification

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## Not yet implemented

- In-place mutable physical B+tree page splits and merges with WAL-protected page images.
- Dirty-page buffer pool, WAL-before-data flush coordinator and torn-page recovery.
- Online checkpoint without blocking the row-store mutex.
- Full multi-table SQL transactions, MVCC, distributed commit and isolation.
- Efficient disk-first index semantics without the full authoritative row map in memory.
- SQL type-aware index ordering, collation versioning and secondary unique indexes.

**Do not classify G7 as a completed physical write-back buffer manager or fully mutable disk B+tree.** It is a working incremental logical index with WAL-backed durability and physical B+tree fallback; production acceptance remains open.
