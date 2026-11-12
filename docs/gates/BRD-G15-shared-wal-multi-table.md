# BRD-G15 — Shared-WAL atomic multi-table transactions

**Normative requirements:** Bradian Database Systems Master Architecture & SRS v1.1  
**Architecture:** [ADR 0020](../adr/0020-shared-wal-multi-table-atomicity.md)  
**Implementation:** `crates/bradian-relational/src/transaction_domain.rs`  
**Status:** Source and integration tests committed directly to `main`. Manual acceptance and power-loss qualification remain open.

## Delivered

- [x] Explicit `MultiTableStore` with one native RowStore/WAL for multiple registered typed tables.
- [x] Durable, immutable, versioned per-table schema pins keyed by stable TableId.
- [x] Schema pin registration idempotent for identical definitions; mismatched reuse refused.
- [x] Reuse of the established G4 typed row, composite PK and schema validators.
- [x] Consistent initial snapshot and shared WAL LSN captured together.
- [x] Transaction-scoped get/scan with read-your-writes across different tables.
- [x] Staged insert, replace and delete with validation before publication.
- [x] Explicit rollback and implicit rollback when a transaction is dropped.
- [x] Exactly one native synchronized WAL batch for committed cross-table changes.
- [x] Global optimistic conflict rejection when any intervening write/registration advances the domain LSN.
- [x] Bounded 256-key/1MiB WAL batch limits; oversized commits rejected without partial writes.
- [x] Reopen and validate all registered table definitions and stored row identities.
- [x] Entire-domain checkpoint, B+tree indexing, verified local backup and restore.
- [x] Corruption refusal on unregistered row namespaces, malformed typed row values and incompatible table schemas.

## Committed test coverage

`crates/bradian-relational/tests/multi_table_transactions.rs` covers one-LSN multi-table commit and restart, rollback, updates/deletes through checkpoint, cross-table conflicts, schema pin lifecycle, invalid rows, batch bounds, concurrent workers, duplicate keys, read-only snapshots, whole-domain backup and source corruption detection.

These checkboxes indicate committed code and test cases; **they do not represent passing test execution**.

## Manual acceptance and fault injection

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Additional tests must exercise process termination before/during/after WAL fsync, torn records, disk-full writes, operating-system exclusive lock behavior, checkpoint/manifest power loss, backup restore, 256-op bounds and multi-thread contention on supported platforms.

## G15 does not yet cover

- [ ] Migrating existing G14 SQL table directories into one shared-WAL domain.
- [ ] SQL `BEGIN`/`COMMIT` across multiple SQL-resolved tables.
- [ ] Atomic commit of G11 catalog DDL and shared-domain data.
- [ ] SQL-level foreign keys, secondary UNIQUE, CHECK and index maintenance.
- [ ] MVCC, predicate/range locks and configurable SQL isolation levels.
- [ ] Multi-process/wire-protocol clients or cross-node consensus.
- [ ] Unbounded transaction size, physical page write-back or in-place index splitting.

**The G15 storage-domain API is a genuinely shared-WAL atomic multi-table operation boundary, not a distributed coordinator or complete production SQL transaction manager.**

Automatic GitHub Actions remain disabled per owner instruction.
