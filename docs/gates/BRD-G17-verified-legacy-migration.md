# BRD-G17 — Verified offline migration to shared-WAL SQL storage

**Specification:** Bradian Database Systems Master Architecture & SRS v1.1  
**Design:** [ADR 0022](../adr/0022-verified-legacy-sql-migration.md)  
**Implementation:** `crates/bradian-relational/src/legacy_migration.rs`  
**Status:** Functional Rust code and integration tests committed to `main`; compilation, tests and operational qualification remain outstanding.

## Delivered migration slice

- [x] Standalone `migrate_legacy_sql(source_root, destination_root)` API.
- [x] Source G11 catalog identity, schema and legacy typed-table validation.
- [x] Exclusive source catalog and every existing table RowStore lock retained through cutover.
- [x] Reject unknown/orphan legacy table directories and existing destinations.
- [x] Refuse source/destination overlap and previously incomplete staging directories.
- [x] Verified catalog-only export/restore through native G10 storage recovery.
- [x] Register the original stable TableIds and table schema pins in a new G15 shared-WAL domain.
- [x] Insert typed rows into staged domain using bounded 128-row atomic G15 batches.
- [x] Compare all migrated typed rows to original source rows.
- [x] Checkpoint and independently reopen the imported catalog and domain.
- [x] Recheck source table images and LSNs before cutover publication.
- [x] Publish only verified staging through destination directory rename and parent sync on Unix.
- [x] Preserve legacy table data and refuse automatic use of partial migration files.
- [x] Return per-table row counts and legacy WAL LSNs, catalog LSN and shared-domain commit LSN.

These checkboxes denote implementation and source-level test coverage, **not an observed passing build**.

## Regression scenarios

`crates/bradian-relational/tests/legacy_migration.rs` exercises multi-table data import, empty schema-only tables, original data preservation, shared-domain SQL writes and restart, catalog/table lock contention, orphan directory refusal, existing destination and repeated cutover refusal, source/staging overlap and incomplete staging rejection.

## Manual acceptance commands

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Additional required operational acceptance: run under a maintenance window with legacy clients stopped; verify catalog and table counts for large source databases; test full disk and corrupt row failure; force termination during source lock acquisition, catalog backup, each import batch, checkpoint, verification and rename; qualify directory sync/rename on deployment filesystems; verify a known-good source backup and a tested rollback plan.

## Still outside G17

- [ ] Online no-downtime legacy migration, change-data capture or dual-write cutover.
- [ ] Resumable, bounded-memory large-scale imports and per-chunk checksummed receipts.
- [ ] Cross-database connection switchover and automatic failback.
- [ ] Atomic catalog+data schema lifecycle DDL and database-wide backup snapshots.
- [ ] MVCC, secondary constraints, foreign-key enforcement and general SQL optimizer.
- [ ] Encryption, multi-tenant authorization, remote SQL and cluster consensus.

**A completed G17 cutover is a verified new database directory, not a claim that legacy files were destroyed or migrated in place.** The catalog backup may checkpoint the source catalog; committed logical metadata and legacy row contents remain intact. Automatic CI stays disabled.
