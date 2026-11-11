# DAP — Implementation Requirements Traceability Matrix

Source of truth: `docs/DER_Analytics_Platform_Architecture_and_Feature_Specification.md`
(spec DER-ANL-PLAT-001 v1.0).

Status legend: `TODO` not started · `WIP` in progress · `DONE` implemented and verified ·
`PART` partially implemented (details in notes) · `DEFER` deferred to a later gate.

## Spec §14 register

| ID | Priority | Requirement (summary) | Status | Crate(s) | Verification |
|---|---|---|---|---|---|
| STO-001 | M | Native versioned `.derc` column segment format | WIP | `dap-storage` | codec/version roundtrip, corruption fuzz |
| STO-002 | M | WAL, atomic manifest, crash recovery | TODO | `dap-storage` | kill/restart at commit boundaries |
| STO-003 | M | Typed pages, NULL masks, dictionaries, compression | WIP | `dap-storage` | golden datasets, numerical parity |
| STO-004 | M | Snapshot-consistent delta/segment reads | TODO | `dap-storage` | concurrent read/write tests |
| STO-005 | M | Stable PK, revision fences, upsert/delete | TODO | `dap-storage` | out-of-order + duplicate replay |
| STO-006 | M | Safe compaction + tombstone GC | TODO | `dap-storage` | reader/snapshot preservation |
| STO-007 | M | Partition/segment/page stats + pruning | TODO | `dap-storage`, `dap-planner` | explain scan-byte assertions |
| STO-008 | A | Adaptive indexes + storage tiering | DEFER | `dap-storage` | measured cost/benefit |
| CAT-001 | M | Versioned DHIS2 semantic catalog | WIP | `dap-catalog` | metadata fixtures + migration |
| CAT-002 | M | Dependency + lineage graph | TODO | `dap-catalog` | impact analysis on field change |
| CAT-003 | M | Query consistency + provenance contracts | WIP | `dap-core`, `dap-planner` | watermark / partial response tests |
| ING-001 | M | Durable source adapters + canonical changes | TODO | `dap-ingest` | source-to-segment integration |
| ING-002 | M | Idempotent, ordered, checkpointed processing | TODO | `dap-ingest` | retry/replay/fault matrix |
| ING-003 | M | Bounded buffering, backfill, quarantine | TODO | `dap-ingest` | high-rate ingest + invalid input |
| RUN-001 | M | Typed vector batches + ownership | WIP | `dap-runtime` | scalar oracle equivalence |
| RUN-002 | M | Scan, filter, project, hash aggregate, sort | WIP | `dap-runtime` | query golden corpus |
| RUN-003 | M | Resource budgets, spill, cancel, fairness | TODO | `dap-runtime` | low-memory co-load |
| RUN-004 | M | Parallel single-node execution | TODO | `dap-runtime` | deterministic concurrency |
| RUN-005 | A | Joins, windows, runtime filters | DEFER | `dap-runtime` | join oracle + spill + skew |
| OPT-001 | M | Logical rewrites + pruning | TODO | `dap-planner` | property-based equivalence |
| OPT-002 | M | Cardinality/cost stats + EXPLAIN | TODO | `dap-planner` | estimate/actual comparison |
| OPT-003 | A | CBO joins, MV rewrite, adaptive | DEFER | `dap-planner` | comparative bench + rollback |
| INC-001 | M | Incremental dependency-based materialization | TODO | `dap-views` | old/new contribution tests |
| INC-002 | M | Period/OU correction + deletion retractions | TODO | `dap-views` | exact count assertions |
| INC-003 | M | Targeted recompute for non-invertible states | TODO | `dap-views` | distinct/cohort tests |
| INC-004 | M | Freshness + invalidation metadata | TODO | `dap-views`, `dap-serving` | consumer lag tests |
| DHI-001 | M | Aggregate (option combos/period/OU) semantics | TODO | `dap-dhis2` | DHIS2 parity fixtures |
| DHI-002 | M | Event + Tracker fact models | TODO | `dap-dhis2` | multi-enrollment/stage fixture |
| DHI-003 | M | Program indicator expression compiler | TODO | `dap-dhis2` | versioned parity suite |
| DHI-004 | M | Relationship + cohort measures | TODO | `dap-dhis2` | reference datasets |
| DST-001 | M | Autonomous site analytics without WAN | TODO | `dap-distributed` | offline reboot suite |
| DST-002 | M | Secure edge-server reconciliation | TODO | `dap-distributed`, `dap-ingest` | reordered WAN replay |
| DST-003 | A | Distributed query fragments | DEFER | `dap-distributed` | multi-worker correctness |
| DST-004 | A | Safe merging + residency policy | DEFER | `dap-distributed`, `dap-security` | cross-site distinct/privacy |
| API-001 | M | Typed, versioned secure query API | WIP | `dap-serving` | contract + load + authz |
| API-002 | M | Data product + freshness + drilldown APIs | TODO | `dap-serving` | consumer integration |
| API-003 | M | Async jobs, cancel, bounded export | TODO | `dap-serving` | large dataset + revocation |
| SEC-001 | M | Row/column/OU/program/site policy | TODO | `dap-security` | cross-role/site matrix |
| SEC-002 | M | Context-safe caching + diagnostics | TODO | `dap-security`, `dap-serving` | leakage tests |
| SEC-003 | M | Encryption + governed deletion + export audit | TODO | `dap-security` | security + retention tests |
| OPS-001 | M | Metrics, tracing, profiling, query history | WIP | `dap-admin`, `dap-serving` | operator diagnostic E2E |
| OPS-002 | M | Capture isolation + bounded background | TODO | `dap-runtime`, `dap-admin` | Capture latency co-load bench |
| OPS-003 | M | Snapshot restore + source-log rebuild | TODO | `dap-storage`, `dap-admin` | DR drill |
| OPS-004 | M | Modern full-screen DER Admin app | DEFER | (out-of-tree) | UI E2E + a11y |
| OPS-005 | M | Reproducible benchmarking + regression CI | WIP | `dap-benchmark` | raw artifact generation |
| OPS-006 | A | Automated index/view/stat tuning | DEFER | `dap-planner`, `dap-storage` | controlled A/B |

## Gate schedule

| Gate | Focus | Status |
|---|---|---|
| G0 | Audit, traceability, ADRs, workspace bootstrap | WIP |
| G1 | Types, catalog, scalar oracle, query DSL slice | TODO |
| G2 | `.derc` storage, WAL, manifests, delta, compaction | TODO |
| G3 | Vectorized operators + spill | TODO |
| G4 | Ingest + materialized views + retractions | TODO |
| G5 | Indexes, stats, CBO, joins, parallelism | TODO |
| G6 | DHIS2 aggregate/event/tracker semantics | TODO |
| G7 | Distributed/federated execution + reconciliation | TODO |
| G8 | Serving + data products + admin | TODO |
| G9 | Production qualification | TODO |

Each gate's exit evidence will be written to `docs/gates/G<N>.md` as it lands.
