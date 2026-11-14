# Bradian implementation gap register

Updated 13 November 2026. Scope and evidence refer to the checked implementation,
not blanket completion of the master SRS. Automatic CI remains manual-only.

## Fixed roadmap and earliest remaining work

Use [Master Gates 0–9](BRADIAN_MASTER_ROADMAP.md). Gate 0 evidence now includes the
complete requirement matrix, source/format inventory and threat model. Open global
ordering and authorization prototypes remain explicit blockers before their formats
are frozen. Gate 1 is the active kernel implementation gate, not master Gate 44.

| Master gate/task | Remaining gap | Required next evidence |
|---|---|---|
| 1 / G1-MEMORY | Query/history/staging, backing/recovery maps, build/metadata buffers, CLI lines, per-domain historical SQL/fallible-native admission, row-store + legacy-migration directory listings and decoded persistent-catalog state are controlled; other directory sites (domain/lifecycle, user data), I/O/cache, cross-domain totals, alternate engines, other output and global admission/cancellation remain incomplete | Global runtime limits and memory-pressure/failure qualification; see [latest evidence](BRADIAN_GATE1_CATALOG_LIMITS_VERIFICATION_2026_11_14.md) |
| 1 / G1-LOCAL-ACCESS | Shared `bradian_relational::local_access` enforces POSIX private-root invariants (symlink refusal, mode 0o700, non-directory refusal, bounded create-if-missing) across the native SQL CLI and future library callers; owner-UID, POSIX ACLs, parent-path trust, Windows ACL and enterprise identity remain unqualified | Owner-UID/ACL/path policy, Windows ACL and enterprise identity; see [latest evidence](BRADIAN_GATE1_LOCAL_ACCESS_VERIFICATION_2026_11_14.md) |
| 1 / G1-FAULTS | Zero-padded WAL extension, zero-filled CRC body, `.tmp` orphans at higher LSNs, stray incidental files and leftover pending manifests are covered; real ENOSPC/EIO propagation, fsync-on-power-loss, device-level torn writes and Windows/macOS qualification remain open | Deterministic I/O fault matrix and platform recovery runs; see [latest evidence](BRADIAN_GATE1_FAULT_RECOVERY_VERIFICATION_2026_11_14.md) |
| 1 / G1-REFERENCE | A single-file positive/negative SQL subset corpus is published (`docs/reference/BRADIAN_SQL_SUBSET.md`) and each claim is backed by a passing test; external SQL-92/99/2016 reference-suite compliance, full documented isolation qualification and Windows/macOS installer/lock tests remain open | External SQL reference-suite compliance, installation qualification; see [latest evidence](BRADIAN_GATE1_SQL_SUBSET_CORPUS_VERIFICATION_2026_11_14.md) |

The canonical local SQL command and Linux development installer close only the
G1-CLI entry-point task. They do not accept Gate 1. New commits stay within the
current master gate until its exit conditions pass.

## Verified implementation tasks (historical record IDs)

| Gap | Implemented behavior | Evidence |
|---|---|---|
| Epoch codec disconnected from SQL | Explicit G35 capability transition; mixed BRRW/BRVE reads and writes | [G35](../gates/BRD-G35-live-schema-epoch-rows.md), twelve tests |
| Catalog revision cannot activate against history | Exclusive nullable-column ALTER with a durable cross-WAL decision | [G36](../gates/BRD-G36-recoverable-historical-alter.md) |
| Historical rows bound only to current schema | Immutable schema timeline, commit-time validation and NULL projection | G36 SQL and corruption tests |
| AS OF columns resolved against latest schema | SELECT * / named-column binding and DESCRIBE use the requested schema epoch | G36 historical query tests |
| Interrupted catalog/history activation unrecoverable | Exact-state roll-forward at all four durable boundaries, owner fencing and idempotent reopen | G36 recovery tests and sixteen torn-frame cases |
| Old inactive intent protocol blocks upgrade | Explicit digest-bound retirement preserving archival records and catalog horizon | [G37](../gates/BRD-G37-legacy-ddl-retirement.md), six tests |
| Backup manifest samples later source statistics | Receipt and statistics captured under one mutation lock | [G38](../gates/BRD-G38-consistent-history-backup-metadata.md), two tests |
| Restore did not enforce the DDL catalog horizon | Full G30/G36 horizon validation before publication | History backup verifier and schema restore tests |
| Non-key NOT NULL relaxation unavailable | Checked DROP NOT NULL through G36 recovery, with earlier schema/value preservation | [G39](../gates/BRD-G39-nullability-relaxation.md) |
| Every INSERT field required after nullable additions | Omitted nullable fields receive NULL; missing required fields still abort the SQL batch | G39 insertion and rollback tests |
| Constant defaults unavailable for added columns | Immutable typed defaults, required additions, old-row projection and INSERT DEFAULT | [G40](../gates/BRD-G40-immutable-column-defaults.md), recovery and compatibility tests |
| Transaction cannot recover part of staged work | Bounded SAVEPOINT, ROLLBACK TO and RELEASE with failed-state recovery and preserved conflict checks | [G41](../gates/BRD-G41-history-savepoints.md), native, SQL and process-exit tests |
| Point reads are never validated at commit | Opt-in bounded read validation, including absent keys, scans and savepoint observations | [G42](../gates/BRD-G42-transaction-read-validation.md), concurrent and twenty-schedule tests |
| Historical UPDATE accepts literals only | Bounded checked arithmetic, same-type column references and standalone DEFAULT, evaluated from the original row | [G43](../gates/BRD-G43-checked-update-expressions.md), numeric corpus and transaction tests |
| Historical SELECT cannot order or page results | Typed ORDER BY, explicit NULL placement and bounded LIMIT/OFFSET with deterministic PK ties | [G44](../gates/BRD-G44-ordered-query-windows.md), paging, conflict and restore tests |

## Remaining implementation and qualification

| Priority | Gap | Required next evidence |
|---|---|---|
| P0 | Broader schema evolution | Default expressions and SET/DROP DEFAULT, SET NOT NULL/backfill, DROP/RENAME/type changes, dependency planning and recovery tests |
| P0 | Concurrent online DDL | Schema leases/reader fencing and explicit old-writer commit semantics; current ALTER requires exclusive mutable ownership |
| P0 | SQL conformance and transaction semantics | Joins, general predicates, expression ordering/collations and general expressions outside the G43 UPDATE subset, secondary/foreign-key enforcement, full isolation-level qualification and reference schedules; ordinary snapshot mode still permits write skew, while G42 validates reads only when opted in |
| P0 | Enterprise access and transport | Individual sessions, SQL authorization, revocation, tenant isolation and TLS/mTLS; current bootstrap gate is loopback-only |
| P0 | Operational recovery qualification | Real power-loss, disk-full, torn-device writes, Windows/macOS tests and long-running restore rehearsals |
| P1 | Mutable physical storage | WAL-protected dirty pages and buffer management; existing on-disk B+tree checkpoints remain immutable |
| P1 | HTAP | Commit-consistent column projections and row/column snapshot/freshness semantics |
| P1 | Strong distributed SQL | Consensus, shard ownership, fencing, distributed transactions and partition/rebalancing tests |
| P1 | Client and Studio integration | Service/wire protocol for relational sessions, drivers and a secured administration workflow; Rust APIs are not a public SQL server |
| P1 | Recovery/metadata growth | Schema history compaction with reference proofs, remote/PITR/incremental archives and workload benchmarks |

[Current executed verification](BRADIAN_GATE1_SINGLE_TABLE_LIMITS_VERIFICATION_2026_11_13.md) records the test
scope and limits. These open requirements must not be presented as production-ready
features or silently enabled through fallback storage engines.
