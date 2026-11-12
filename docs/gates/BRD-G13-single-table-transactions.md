# BRD-G13 — Single-table atomic transactions and optimistic conflict validation

**Specification:** Bradian Database Systems Master Architecture & SRS v1.1  
**Architecture:** [ADR 0018](../adr/0018-single-table-transactions.md)  
**Implementation:** `crates/bradian-relational/src/typed_table.rs`, `row_store.rs` and `sql.rs`  
**Status:** Source and integration tests committed to `main`; compilation, test execution and platform qualification pending.

## Implemented transaction capability

The typed-table API now supports a real local unit of work: `begin_transaction`, staged INSERT/replace/DELETE, stable snapshot reads, read-your-writes, explicit `rollback`, and atomic `commit`. A single WAL frame represents all staged final mutations, and the native `RowStore` validates the initial LSN under the same lock as WAL append. A stale transaction fails as a whole, without exposing partial changes.

The `SqlEngine::transact_table` helper resolves a fully qualified SQL table and accepts an application-defined typed transaction closure. On success it commits once and returns a durable LSN. A closure error cancels all pending changes. This is an application API, not parsing of SQL transaction-control statements.

## Regression tests committed

- [x] Multiple-row commit survives restart and consumes one WAL sequence.
- [x] Rollback and uncommitted transaction drop do not write WAL.
- [x] Two-key transfer becomes visible together.
- [x] Concurrent stale transaction fails without altering committed data.
- [x] Unrelated-key writes produce conservative whole-table conflict.
- [x] Stable snapshot reads and read-your-writes.
- [x] Delete/reinsert of the same key resolves correctly.
- [x] Rejected row validation does not append partial changes.
- [x] More than 256 operations fails the entire batch.
- [x] Multiple threads contend without publishing partial transactions.
- [x] Tombstones persist through checkpoint and restart.
- [x] SQL-resolved transaction helper commits a multi-row batch.
- [x] Callback failure rolls back and invalid table names fail resolution.

Checkboxes describe **committed test cases**, not observed passing runs.

## Required manual acceptance

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Crash-injection at the WAL append/sync boundary, disk-full behavior, concurrent read/write stress and cross-platform fsync qualification remain required.

## Not yet implemented

- [ ] SQL `BEGIN`, `COMMIT`, `ROLLBACK` sessions.
- [ ] Cross-table or catalog-plus-table atomic commits.
- [ ] MVCC row versions, nonblocking snapshot reads and predicate locks.
- [ ] Physical page WAL redo/undo and transaction-safe mutable B+tree pages.
- [ ] SQL SELECT FOR UPDATE, savepoints, isolation level configuration.
- [ ] Distributed transaction coordination or serializable cluster-wide SQL.

**This gate covers functional local single-table atomic transactions**, not complete ACID SQL across the Bradian database. Automatic CI remains disabled by the owner.
