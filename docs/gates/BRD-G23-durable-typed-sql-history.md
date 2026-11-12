# BRD-G23 — Durable MVCC typed SQL and verified shared-domain migration

**Specification:** Bradian Database Systems Master Architecture & SRS v1.1  
**Architecture:** [ADR 0028](../adr/0028-durable-typed-sql-history.md)  
**Implementation:** `crates/bradian-relational/src/durable_typed.rs`, `history_sql.rs`, `history_migration.rs`  
**Status:** Source and regression tests committed to `main`; independent Cargo build, test execution and operational qualification outstanding.

## Committed G23 scope

- [x] Separate `HistorySqlEngine` over the G22 `history-domain/`, preserving all older G15/G16/G21 formats.
- [x] Reuse G11 durable catalog, G16 SQL grammar, G15 typed row/primary key encoding and G22 committed history/tombstones.
- [x] Validate every registered durable schema pin against the catalog and every stored row against typed schema and primary key identity.
- [x] Typed SQL BEGIN, COMMIT, ROLLBACK and cross-table INSERT/SELECT/UPDATE/DELETE through one historical WAL.
- [x] Retained historical SQL `SELECT ... FROM db.schema.table AS OF <unsigned WAL LSN> [WHERE full primary key]`.
- [x] Historical point reads and full-table scans based on committed G22 row versions, surviving checkpoint and process restart.
- [x] Refuse historical queries before matching table schema registration, below the durable retention floor or beyond latest committed LSN.
- [x] Cross-table atomic commit in one synchronized BRWL history batch, with G22 per-key conflict detection.
- [x] Exact unsigned 64-bit WAL LSN parser, rejecting negative, fractional and malformed values.
- [x] `migrate_shared_to_history` implements an offline copy into a separate, never-overwritten staging/cutover root.
- [x] Acquire original G11/G15 catalog/domain file locks; verify all stable TableIds and schema pins against the authoritative catalog.
- [x] Use G10 verified catalog copy and bounded G22 typed row import batches.
- [x] Independently reopen and compare destination schemas, row counts and full typed content before publication.
- [x] Preserve the legacy shared-domain root and refuse unsafe overlapping, occupied, unregistered or previously incomplete destinations.
- [x] Explicitly reject G18-managed sources and unmigrated G14 table storage until a lifecycle-aware migration is implemented.
- [x] Refuse G23 history roots in the G12, G16, G21 and G18 legacy owners so they cannot accidentally create competing physical stores.
- [x] Eleven source-level integration tests committed, covering migration, historical SQL, WAL recovery, rollback and refusal modes.

Checkmarks denote committed source and test cases, **not observed passing test execution**.

## Required manual validation

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Operational acceptance additionally requires comparison of restored counts and typed values on a known dataset, 498-byte physical-key and 3545-byte row-value boundary tests, crash injection during schema-pin import and each G22 row batch, checkpoint corruption tests, source lock contention, disk-full behavior, rename/fsync qualification, source rollback drills and large-scale import benchmarks.

## Deliberate outstanding work

- [ ] G18 lifecycle-managed source conversion and G19 coordinated database-wide backup for the G23 history layout.
- [ ] Carrying forward historical versions from an earlier source (G16 stores latest rows only).
- [ ] Immutable historical schema versions across ALTER/DROP; catalog-plus-history atomic DDL.
- [ ] Production-grade SQL optimizer, joins, predicates and historical analytics.
- [ ] Version-key physical indexing, bounded memory admission and automated safe retention policies.
- [ ] Serializable SQL read-set/predicate tracking and write-skew protection.
- [ ] Remote SQL sessions, encryption, authentication and cluster durability.
- [ ] Certified manual build/test results and destructive failure-injection verification.

**G23 is a separate, opt-in, bounded durable-history SQL mode and offline import, not an automatic upgrade of the existing production SQL engine.**

Automatic GitHub Actions remain manual-only as directed.
