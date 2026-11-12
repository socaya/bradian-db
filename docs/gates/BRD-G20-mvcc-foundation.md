# BRD-G20 — In-memory MVCC version visibility over durable row WAL

**Specification:** Bradian Database Systems Master Architecture & SRS v1.1  
**Design:** [ADR 0025](../adr/0025-mvcc-row-version-foundation.md)  
**Implementation:** `crates/bradian-relational/src/mvcc.rs`  
**Status:** Source and regression tests committed on `main`; manual compiler/test and fault-injection acceptance still outstanding.

## Delivered

G20 introduces an opt-in single-RowStore MVCC API with pinned in-memory version chains. It retains old values while active readers need them, allows disjoint-key writers to commit from an older snapshot, and rejects stale writes to a key with a newer committed version. All committed latest values remain durable via the existing G3 BRWL atomic batch and BRPG/BRCP checkpoint protocol.

- [x] `MvccStore::open` reconstructs the latest committed state from native WAL recovery.
- [x] `begin()` captures and pins the current LSN without cloning the complete row map.
- [x] Snapshot `get` returns the newest row version visible at transaction start.
- [x] Snapshot `scan` returns ordered visible rows merged with staged writes.
- [x] Staged INSERT/replace/DELETE and tombstones with read-your-writes.
- [x] Rollback and dropped transactions leave WAL and visible state unchanged.
- [x] Per-key optimistic first-committer-wins, including delete/reinsert ABA.
- [x] Concurrent disjoint-key writers can both commit.
- [x] Full-table scan transactions conservatively detect intervening mutations.
- [x] Committed changes across keys appear through one synchronized native WAL batch.
- [x] Reference-counted snapshot pins and safe old-version reclamation.
- [x] Snapshot/retention stats: committed LSN, readers, positions, keys and versions.
- [x] Checkpoint and restart preserve the latest committed row image.
- [x] Fourteen regression tests committed.

The above checkboxes mean functionality and tests are **present in source**, not that the tests have executed successfully.

## Manual acceptance

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Fault injection: WAL append/sync interruption, crash during checkpoint, tombstone/ABA scenarios, contention with pinned readers, high version-retention pressure, 256-operation atomic batch limits, and filesystem recovery.

## Not yet implemented

- [ ] Binding G20 into the G15/G16 multi-table SQL engine.
- [ ] On-disk persistent historical row versions, restartable old snapshots or time-travel queries.
- [ ] Serializable SQL isolation, predicate locks and protection against write skew.
- [ ] MVCC physical B+tree/index visibility or page-version garbage collection.
- [ ] Memory admission, spill-to-disk history, transaction limits and timeouts.
- [ ] Durable schema-version binding for concurrent SQL DDL/DML.
- [ ] Distributed snapshot tracking and cluster-wide transaction isolation.

**This is an independent, WAL-backed row-level MVCC foundation. Existing SQL execution remains unchanged**, with its earlier conservative G15 conflicts. Automatic GitHub CI remains disabled per owner request.
