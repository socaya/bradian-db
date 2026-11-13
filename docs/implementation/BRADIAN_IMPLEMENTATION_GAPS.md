# Bradian implementation gap register

Updated 13 November 2026. Scope and evidence refer to the checked implementation,
not blanket completion of the master SRS. Automatic CI remains manual-only.

## Closed through G39

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

## Remaining implementation and qualification

| Priority | Gap | Required next evidence |
|---|---|---|
| P0 | Broader schema evolution | Non-NULL defaults, SET NOT NULL/backfill, DROP/RENAME/type changes, dependency planning and recovery tests |
| P0 | Concurrent online DDL | Schema leases/reader fencing and explicit old-writer commit semantics; current ALTER requires exclusive mutable ownership |
| P0 | SQL conformance and transaction semantics | Joins, expressions, secondary/foreign-key enforcement, savepoints and serializable schedule tests; current MVCC can admit write skew |
| P0 | Enterprise access and transport | Individual sessions, SQL authorization, revocation, tenant isolation and TLS/mTLS; current bootstrap gate is loopback-only |
| P0 | Operational recovery qualification | Real power-loss, disk-full, torn-device writes, Windows/macOS tests and long-running restore rehearsals |
| P1 | Mutable physical storage | WAL-protected dirty pages and buffer management; existing on-disk B+tree checkpoints remain immutable |
| P1 | HTAP | Commit-consistent column projections and row/column snapshot/freshness semantics |
| P1 | Strong distributed SQL | Consensus, shard ownership, fencing, distributed transactions and partition/rebalancing tests |
| P1 | Client and Studio integration | Service/wire protocol for relational sessions, drivers and a secured administration workflow; Rust APIs are not a public SQL server |
| P1 | Recovery/metadata growth | Schema history compaction with reference proofs, remote/PITR/incremental archives and workload benchmarks |

[Current executed verification](BRADIAN_G39_VERIFICATION_2026_11_13.md) records the test
scope and limits. These open requirements must not be presented as production-ready
features or silently enabled through fallback storage engines.
