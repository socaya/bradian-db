# BRD-G22 — WAL-durable MVCC history and bounded retention

**Specification:** Bradian Database Systems Master Architecture & SRS v1.1  
**Architecture:** [ADR 0027](../adr/0027-durable-mvcc-history.md)  
**Implementation:** `crates/bradian-relational/src/durable_mvcc.rs`  
**Status:** Source, documentation and regression tests committed directly to `main`; compilation, test execution and crash-injection qualification outstanding.

## Implemented gate

- [x] New explicit `BRADIAN-DURABLE-MVCC-V1` storage namespace; incompatible stores fail closed.
- [x] Each immutable historical row version is keyed by its user key and 64-bit WAL commit LSN.
- [x] Versioned values and deletion tombstones are persisted in one synchronized BRWL batch per transaction.
- [x] Full transaction rollback leaves neither current nor historical records partially committed.
- [x] Historical point read by `read_as_of(key, lsn)` is recovered from committed history keys after restart.
- [x] Concurrent writes to disjoint keys can commit from the same starting snapshot.
- [x] Same-key first-committer-wins conflict detection, including delete/reinsert behavior.
- [x] Conservative full-scan optimistic conflict validation.
- [x] Pinned snapshot horizons and read-your-writes.
- [x] Explicit, durable retention floor and refusal of historical reads below the floor.
- [x] No retention advancement past an active reader's pinned snapshot.
- [x] Retain the newest version at/below the floor plus newer versions for correct as-of semantics.
- [x] Advance floor before deleting superseded versions; interrupted pruning is recoverable/retryable.
- [x] Garbage collect old history keys in bounded 256-operation WAL delete batches.
- [x] G3 WAL and G9 BRPG/BRCP checkpoints provide restart recovery of retained historical versions.
- [x] G10 independently verified backups capture history, tombstones and retention floor.
- [x] Reject invalid version markers, physical key namespaces, unexpected version lengths and out-of-range user keys/values.
- [x] Preserve G20/G21 and existing G15/G16 SQL storage unchanged; no silent format conversion.
- [x] Seventeen integration tests committed for durability, conflicts, retention, integrity and backup.

These checkmarks describe committed implementation and regression cases, **not a passing compilation or test report**.

## Manual acceptance

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Additional required qualification: controlled process termination before/during/after each BRWL batch fsync, torn final WAL frames, checkpoint publication interruption, disk-full writes, retention-floor publication interruption, pruning interruption between chunks, 256-operation boundary, tombstone ABA tests, long-lived-reader memory pressure, large-history checkpoint/backup/restore, and platform-specific directory synchronization.

## Remaining development gates

- [ ] G22 durable historical row format integration into typed table and G21 multi-table SQL execution.
- [ ] Verified conversion from G15/G21 current-row physical keys to G22 history keys without losing committed values.
- [ ] Durable schema-version identity, consistent `AS OF` SQL queries and historical relational constraints.
- [ ] Row-version indexes, page/version compression, bounded memory and retention policy scheduling.
- [ ] Serializable SQL read-set/predicate tracking and write-skew prevention.
- [ ] G19 database-wide backup integration for versioned SQL history.
- [ ] Cryptographic archive authenticity, encryption, remote clients and cluster-wide history coordination.

**G22 provides durable historical point reads via an independent versioned native RowStore. It does not yet make the G21 SQL engine persistent-history MVCC.** Automatic CI remains disabled as directed.
