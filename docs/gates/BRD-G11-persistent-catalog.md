# BRD-G11 — WAL-backed persistent relational metadata catalog

**Specification:** Bradian Database Systems Master SRS v1.1  
**Architecture:** [ADR 0016](../adr/0016-persistent-ddl-catalog.md)  
**Code:** `crates/bradian-relational/src/persistent_catalog.rs`  
**Status:** Source implementation and 12 regression tests committed directly to `main`; execution-based acceptance not yet complete.

## Implemented functionality

The relational catalog now has a durable, exclusively locked DDL journal backed by Bradian's native `RowStore`. Its `PersistentCatalog` API accepts validated database, schema, table and foreign-key metadata operations and records each operation in one synchronized `BRWL` event **before** publishing the new state. Restart rebuilds the same object graph and stable monotonic identities by replaying commands through the original in-memory Catalog validators.

The implementation records a version marker, validates exact journal ordering, verifies identity allocation and rejects malformed events or unauthorized key namespaces. It supports underlying BRPG/BRCP checkpointing and maintains RESTRICT dependency semantics through deletion and restart.

## Committed regression tests

- [x] Stable database, schema and table identifiers after restart.
- [x] Duplicate and invalid DDL rejection without WAL commit or identity consumption.
- [x] Foreign-key validation and RESTRICT deletion across checkpoint/restart.
- [x] No identifier reuse after dropping objects.
- [x] Replay of post-checkpoint DDL WAL frames.
- [x] Concurrent conflicting DDL: one successful creation.
- [x] Exclusive directory ownership.
- [x] Corrupted event fails closed.
- [x] Unknown journal keys rejected.
- [x] Invalid catalog version marker rejected.
- [x] Oversized schema DDL rejected without committing.
- [x] Read snapshots remain independent from later mutations.

These checkboxes describe **tests present in source**, not observed passing tests.

## Manual acceptance

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Additional fault injection must target WAL append/sync failures, corrupt event replays, interrupted checkpoint publication, store process termination, disk-full handling and power-loss recovery on Linux, macOS and Windows.

## Not yet implemented

- Multi-DDL-statement transactions and transaction-aware metadata/row-store coordination.
- Native SQL DDL parsing, ALTER TABLE migration, transactional DROP that affects stored rows.
- Catalog compaction and incremental startup for very large DDL histories.
- Unified catalog+tables backup/restore and cluster-consistent DDL.
- Role-based SQL permissions, schema version pinning for running transactions, MVCC and distributed coordination.
- Independently qualified cryptographic integrity and at-rest encryption.

**G11 is operational metadata persistence, not a fully transactional SQL catalog release.** Automatic CI remains disabled at the owner's request.
