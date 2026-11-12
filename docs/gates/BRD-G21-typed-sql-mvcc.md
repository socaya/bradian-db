# BRD-G21 — MVCC Integration with Typed Tables and SQL

**Specification:** Bradian Database Systems Master Architecture & SRS v1.1  
**Architecture:** [ADR 0026](../adr/0026-typed-sql-mvcc-integration.md)  
**Implementation:** `crates/bradian-relational/src/typed_mvcc.rs` and `src/mvcc_sql.rs`  
**Status:** Source and nine regression tests committed to `main`; independent compilation, test execution and fault injection remain outstanding.

## Implemented capabilities

- [x] `TypedMvccStore` binds G20's versioned row engine to G15 BRPK/BRRW typed physical rows and original stable TableIds.
- [x] Startup verifies domain schema pins against the G11 authoritative catalog and checks every physical row's typed decoding and primary-key identity.
- [x] Missing/mismatched/unregistered schemas and legacy G14 table storage are rejected; no silent migration.
- [x] G18 lifecycle reconciliation occurs before taking SQL storage ownership when a lifecycle journal exists.
- [x] `MvccSqlEngine` is a distinct opt-in engine; G16 SQL behavior is unchanged by default.
- [x] G16 parser, binder, full-primary-key predicates and typed SQL results are reused.
- [x] Multiple concurrent scoped SQL sessions can hold independently pinned MVCC snapshots.
- [x] SQL `BEGIN`, `COMMIT`, `ROLLBACK`, INSERT, SELECT, UPDATE and DELETE across registered shared-domain tables.
- [x] Read-your-writes, stable point reads and full-table snapshot scans.
- [x] Disjoint-key concurrent writers can both commit; same-key stale writers fail.
- [x] Full physical scans conservatively reject write commits after intervening domain changes.
- [x] Atomic multiple-table WAL commits using the existing 256-operation/1 MiB native batch limits.
- [x] Historical row versions retained in RAM while readers need them; reclaimed when unpinned.
- [x] G15/G16 physical on-disk compatibility: after closing G21, the existing G16 engine can read the same committed rows.
- [x] Latest WAL-backed rows and table schemas survive checkpoint/restart without claiming historical snapshot recovery.

**These checkmarks describe committed code and test cases, not observed passing tests.**

## Regression cases

`tests/mvcc_sql.rs` includes nine tests: cross-table SQL commit/restart and G16 interoperability, overlapping snapshot visibility, concurrent disjoint-key writers, conflicting same-key updates, scan-phantom conflict, rollback/statement-error abort, unregistered/legacy storage rejection, missing domain refusal, and history retention/restart.

## Manual acceptance

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Additional qualification should stress thousands of simultaneous readers, long-held snapshot memory usage, Rust engine/RowStore lock exclusion, WAL crash injection at a multi-table commit, full-scan conflicts, schema corruption on open, G18 pending recovery, and restore through G19 after G21 closes.

## Not implemented

- [ ] Persistent old row versions or restartable historical snapshots.
- [ ] Serializable SQL, robust predicate/range locks or write-skew protection for point-read transactions.
- [ ] G21 as the default G16 SQL mode, network SQL connections or cross-process concurrent writers.
- [ ] Concurrent online catalog DDL with MVCC schema versioning.
- [ ] Secondary UNIQUE/FK/CHECK enforcement, general SQL predicates, joins and optimizer.
- [ ] Versioned physical page layout, spillable history, back-pressure and limits on snapshot lifetime.
- [ ] Cluster-wide MVCC and consensus.

**G21 is an opt-in typed MVCC SQL path, not a production-certified serializable SQL server.** Automatic CI remains disabled by owner request.
