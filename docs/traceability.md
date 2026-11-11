# DAP — Implementation Requirements Traceability Matrix

Source of truth: `docs/DER_Analytics_Platform_Architecture_and_Feature_Specification.md`
(spec DER-ANL-PLAT-001 v1.0).

Status legend: `TODO` not started · `WIP` in progress · `DONE` implemented and verified ·
`PART` partially implemented (details in notes) · `DEFER` deferred to a later gate.

## Spec §14 register

| ID | Priority | Requirement (summary) | Status | Crate(s) | Verification |
|---|---|---|---|---|---|
| STO-001 | M | Native versioned `.derc` column segment format | DONE | `dap-storage` | roundtrip + corruption + version-reject tests |
| STO-002 | M | WAL, atomic manifest, crash recovery | DONE | `dap-storage` | partition_durability suite covers crash+replay |
| STO-003 | M | Typed pages, NULL masks, dictionaries, compression | DONE | `dap-storage` | codec roundtrips + dict codec selection test |
| STO-004 | M | Snapshot-consistent delta/segment reads | DONE | `dap-storage`, `dap-runtime` | `Partition::snapshot_rows` + scan merges delta |
| STO-005 | M | Stable PK, revision fences, upsert/delete | DONE | `dap-storage` | version-fence + idempotent-replay tests |
| STO-006 | M | Safe compaction + tombstone GC | PART | `dap-storage` | flush-compaction + snapshot refs (multi-seg merge is DEFER) |
| STO-007 | M | Partition/segment/page stats + pruning | PART | `dap-storage`, `dap-planner` | zone maps stored; planner-side pruning DEFER |
| STO-008 | A | Adaptive indexes + storage tiering | DEFER | `dap-storage` | measured cost/benefit |
| CAT-001 | M | Versioned DHIS2 semantic catalog | DONE | `dap-catalog` | registry tests + lineage |
| CAT-002 | M | Dependency + lineage graph | DONE | `dap-catalog`, `dap-views` | transitive_dependents + views DAG test |
| CAT-003 | M | Query consistency + provenance contracts | DONE | `dap-core`, `dap-planner` | Provenance on every QueryResult |
| ING-001 | M | Durable source adapters + canonical changes | DONE | `dap-ingest` | envelope + schema coercion |
| ING-002 | M | Idempotent, ordered, checkpointed processing | DONE | `dap-ingest`, `dap-storage` | e2e acceptance #3 (10× replay = 1 effect) |
| ING-003 | M | Bounded buffering, backfill, quarantine | PART | `dap-ingest` | quarantine log in place; bounded buffers DEFER |
| RUN-001 | M | Typed vector batches + ownership | DONE | `dap-runtime` | scalar oracle equivalence test |
| RUN-002 | M | Scan, filter, project, hash aggregate, sort | DONE | `dap-runtime` | unit + e2e |
| RUN-003 | M | Resource budgets, spill, cancel, fairness | PART | `dap-runtime` | ExecContext + budget guard; disk spill DEFER |
| RUN-004 | M | Parallel single-node execution | DEFER | `dap-runtime` | deterministic concurrency |
| RUN-005 | A | Joins, windows, runtime filters | DEFER | `dap-runtime` | join oracle + spill + skew |
| OPT-001 | M | Logical rewrites + pruning | PART | `dap-planner` | predicate injection done; zone-map pruning DEFER |
| OPT-002 | M | Cardinality/cost stats + EXPLAIN | PART | `dap-planner` | EXPLAIN tree landed; true CBO stats DEFER |
| OPT-003 | A | CBO joins, MV rewrite, adaptive | DEFER | `dap-planner` | comparative bench + rollback |
| INC-001 | M | Incremental dependency-based materialization | PART | `dap-views` | DAG + freshness types ship; incremental refresh DEFER |
| INC-002 | M | Period/OU correction + deletion retractions | PART | `dap-views`, `dap-storage` | delta tombstones hide from scan (e2e acceptance #1) |
| INC-003 | M | Targeted recompute for non-invertible states | DEFER | `dap-views` | distinct/cohort tests |
| INC-004 | M | Freshness + invalidation metadata | PART | `dap-serving`, `dap-views` | /freshness endpoint + FreshnessStamp |
| DHI-001 | M | Aggregate (option combos/period/OU) semantics | DONE | `dap-dhis2`, `dap-planner` | Aggregate DSL + e2e; `aggregate.rs` reduction matrix for SUM/AVG/AVG_SUM_OU/COUNT/MIN/MAX/NONE |
| DHI-002 | M | Event + Tracker fact models | DONE | `dap-dhis2` | `event.rs` EventFact + `tracker.rs` Enrollment/TEI + distinct-TEI + stage-scoped D{} refs |
| DHI-003 | M | Program indicator expression compiler | DONE | `dap-dhis2` | Full expression grammar (24 tests); Event PI (7 tests); Enrollment PI (8 tests); Classic Indicator (10 tests) |
| DHI-004 | M | Relationship + cohort measures | PART | `dap-dhis2` | Longitudinal (`enrollments_in_period`, `completions_in_period`) ship; Relationship facts DEFER (see G6.md step 6) |
| DST-001 | M | Autonomous site analytics without WAN | PART | `dap-storage`, `dap-runtime` | partition runs offline; sync DEFER |
| DST-002 | M | Secure edge-server reconciliation | DEFER | `dap-distributed`, `dap-ingest` | reordered WAN replay |
| DST-003 | A | Distributed query fragments | DEFER | `dap-distributed` | multi-worker correctness |
| DST-004 | A | Safe merging + residency policy | DEFER | `dap-distributed`, `dap-security` | cross-site distinct/privacy |
| API-001 | M | Typed, versioned secure query API | DONE | `dap-serving` | router + HTTP roundtrip tests |
| API-002 | M | Data product + freshness + drilldown APIs | PART | `dap-serving` | freshness landed; drilldown + data products DEFER |
| API-003 | M | Async jobs, cancel, bounded export | DEFER | `dap-serving` | large dataset + revocation |
| SEC-001 | M | Row/column/OU/program/site policy | DONE | `dap-security` | policy_predicate + e2e acceptance test |
| SEC-002 | M | Context-safe caching + diagnostics | DONE | `dap-security` | cache_key fingerprint test |
| SEC-003 | M | Encryption + governed deletion + export audit | PART | `dap-security` | AuditLog in place; encryption DEFER |
| OPS-001 | M | Metrics, tracing, profiling, query history | DONE | `dap-serving` | /engine/health + query history ring |
| OPS-002 | M | Capture isolation + bounded background | DEFER | `dap-runtime`, `dap-admin` | Capture latency co-load bench |
| OPS-003 | M | Snapshot restore + source-log rebuild | PART | `dap-storage` | WAL replay covers runtime restart; snapshot export DEFER |
| OPS-004 | M | Modern full-screen DER Admin app | DEFER | (out-of-tree) | UI E2E + a11y |
| OPS-005 | M | Reproducible benchmarking + regression CI | DONE | `dap-benchmark` | criterion harness + scripts/bench.sh |
| OPS-006 | A | Automated index/view/stat tuning | DEFER | `dap-planner`, `dap-storage` | controlled A/B |

## Gate schedule

| Gate | Focus | Status | Exit evidence |
|---|---|---|---|
| G0 | Audit, traceability, ADRs, workspace bootstrap | DONE | ADR 0001-0003, CLAUDE.md, this file |
| G1 | Types, catalog, scalar oracle, query DSL slice | DONE | `docs/gates/G1.md` |
| G2 | `.derc` storage, WAL, manifests, delta, compaction | DONE | `docs/gates/G2.md` |
| G3 | Vectorized operators + spill | PART | 77-test runtime + bench baseline; disk spill DEFER |
| G4 | Ingest + materialized views + retractions | PART | canonical ingest + delta tombstones + freshness; incremental MV refresh DEFER |
| G5 | Indexes, stats, CBO, joins, parallelism | DEFER | — |
| G6 | DHIS2 aggregate/event/tracker semantics | PART | aggregate + program indicator parser; event/tracker DEFER |
| G7 | Distributed/federated execution + reconciliation | DEFER | — |
| G8 | Serving + data products + admin | PART | serving v1 + CLI stub; data product catalog + admin UI DEFER |
| G9 | Production qualification | DEFER | — |

Deferred items are the scope for the next iteration. Each `PART` row names the specific slice
that still needs work in its verification column.
