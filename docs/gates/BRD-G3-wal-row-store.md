# Bradian G3 — Native WAL-backed row store and checkpoint recovery

**SRS:** Bradian Database Systems v1.1, native row storage and crash recovery  
**Implementation:** `crates/bradian-relational/src/row_store.rs`  
**Design authority:** [ADR 0008](../adr/0008-native-row-wal-checkpoint.md)  
**Milestone:** Operational single-process binary key/value row persistence  
**Gate status:** Code and regression tests committed; manual build and qualification outstanding.

## Functional acceptance scope

This milestone is **working persistence functionality**, not a placeholder. A `RowStore` serializes in-process mutations with `parking_lot::Mutex`, validates keys and inline values, appends one framed mutation to a CRC-protected WAL, synchronizes the WAL before returning success, and keeps an ordered in-memory index for reads. Deletes, replacements and binary keys are supported.

Checkpointing streams sorted records into versioned 4 KiB data pages, beginning with a metadata page containing the checkpoint sequence, page count and row count. Data pages are generated with bounded transient memory, CRC-protected, synchronized and published via a new immutable snapshot filename. The WAL is truncated only after successful snapshot publication, and obsolete snapshots are cleaned up opportunistically.

Recovery validates published pages, reads WAL frames sequentially and replays changes after the checkpoint LSN. It repairs only an incomplete trailing record, rejects a bad checksum or invalid sequence, and preserves prior acknowledged writes when the published snapshot is valid.

## Implemented tests

- [x] Restart replay of inserts, replacements and deletes.
- [x] Atomically committed multi-key WAL batches under one LSN.
- [x] Optional global-LSN optimistic conflict rejection before append.
- [x] Torn final WAL batch rejection without partial application.
- [x] Malformed batch with recomputed CRC rejected before changes.
- [x] Ordered reads with binary keys.
- [x] Exact maximum inline row capacity.
- [x] Multi-page snapshot construction and restart.
- [x] Two-pass streaming page checkpoint boundary handling.
- [x] Checkpoint idempotence and restart after post-checkpoint writes.
- [x] Truncated final WAL header and body repair.
- [x] CRC mismatch and invalid magic failure.
- [x] LSN gap rejection even after recomputed CRC.
- [x] Unpublished snapshot temp file rejection.
- [x] Published snapshot corruption failure.
- [x] Snapshot-before-WAL-truncate crash interval replay.
- [x] Repeated checkpoints and deleted-row preservation.
- [x] Concurrent in-process writer serialization.
- [x] Rejection of invalid keys and overlong values before append.

These represent **committed tests, not executed-test evidence**.

## Acceptance evidence still required

- [ ] Manual `cargo check --workspace --locked` passes.
- [ ] Manual `cargo test -p bradian-relational --locked` passes.
- [ ] Manual `cargo test --workspace --locked` passes.
- [ ] Manual `cargo clippy --workspace --all-targets --locked -- -D warnings` passes.
- [ ] Platform-specific fsync and rename power-loss crash tests for Linux, Windows and macOS.
- [ ] Inter-process exclusive file lock and stale-lock recovery tests.
- [ ] Page and WAL encryption/key custody tests.
- [ ] Disk-full, out-of-space, I/O failures, checksum and corrupt-directory fault injection.
- [ ] Persistent catalog linkage and relational row encoding tests.
- [ ] True on-disk B+tree, free-page allocation and buffer pool implementation.
- [ ] Isolation, MVCC, multi-row commits, crash-safe transactional DDL and SQL semantics.

## Explicit limitation

**G3 is not the full ACID relational DBMS gate.** Single-operation WAL sync, multi-key atomic batch commit and restart recovery are implemented but are not sufficient for production database guarantees, multi-row atomic transactions, inter-process safe concurrent writers or globally consistent snapshots. Keep the storage path exclusive to a single process. The legacy analytical `.derc` format and DHIS2 integrations remain unchanged.

CI automation is disabled by owner instruction. This gate remains **unaccepted** until manual checks and outstanding correctness requirements are verified.
