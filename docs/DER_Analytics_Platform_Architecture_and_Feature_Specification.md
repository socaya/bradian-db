# DER Analytics Platform
## Comprehensive Product, Storage, Compute and Distributed Analytics Architecture

**Document ID:** DER-ANL-PLAT-001  
**Version:** 1.0 | **Date:** 11 November 2026  
**Audience:** DER architects, Claude Code, server/client engineers, data engineers, QA, SRE and security teams  
**Status:** Target architecture and comprehensive implementation requirements (not an assertion of existing capabilities)

## 1. Executive vision

Build a **fully DER-owned, high-performance, cloud-optional, offline-first analytical platform** that delivers native columnar storage, vectorized query execution, incremental computation, cost-based planning, intelligent data products, distributed federation and native DHIS2 semantics. It shall operate on both DER Server and DER Client, with a common semantic and query contract and hardware-adapted runtimes.

**Non-negotiable:** no ClickHouse, Apache Doris, DuckDB or independently operated OLAP database runtime shall be required for core analytical storage or execution. Established open-source *libraries* for compression, cryptography, vectorized primitives, serialization and interchange are acceptable if carefully reviewed, but DER must own the storage format, data catalog, planner, execution orchestration, correctness semantics, APIs and distributed coordination. Existing transactional PostgreSQL may remain the source of truth. Do not rewrite a database solely for branding: make the DER engine uniquely fit its work.

**'Surpass' is a testable goal, not a blanket claim.** DER should beat selected established engines on pre-registered DER-specific workloads where meaningful, and exceed general-purpose engines in DHIS2-native integration, resilient site analytics, edge footprint, correction-aware aggregation and privacy-governed federation. Never assert universal superiority without independently reproducible data.

### 1.1 Guiding principles

1. Transaction processing and analytics are isolated; Capture/Data Entry responsiveness is protected.
2. A common semantics engine serves Server and Client despite distinct execution profiles.
3. Metadata-driven: no hardcoded programs, stages, indicators, periods, organisations or categories.
4. Exactness, access controls, durability and reproducibility precede performance optimization.
5. Streaming corrections, historical retractions, deletion and out-of-order delivery are normal operations.
6. Every result states data freshness, source versions, metadata revision and consistency mode.
7. Full offline operation at facility edge, with secure central consolidation later.
8. Deterministic cost-based planning; AI suggestions may tune but never redefine analytical truth.
9. Observability, profiling, adaptive tuning, recovery and documentation are core product features.
10. One vertical slice must be executable before introducing advanced microservices or clusters.

### 1.2 Core use cases

- National/regional/facility DHIS2 analytical cubes over aggregate reporting data.
- Real-time and near-real-time event tally by organisation, period, stage and dimensions.
- Tracker program indicators, enrollments, longitudinal cohorts, repeated stages, relationships.
- High-concurrency DER Visualiser dashboards, drilldowns and line listings.
- Streaming analytics for distributed service-point workflows, facility queues and operational metrics.
- Site-level analytics during WAN outages and federated/cumulative reporting after reconnection.
- Reusable versioned data products for Reporting, Visualiser, Capture, DER Admin and approved integrations.

---

## 2. Reference architecture

```text
   DER Aggregate Data Entry / Capture / Metadata / Transaction Journal
                               |
                  Durable Ingest + CDC Adapters
                               |
               Canonical Change Log + Validation
                               |
         +------------------+-----------------------+
         | Mutable Delta    | Immutable DER Column  |
         | + Primary Index  | Segments + Indexes    |
         +------------------+-----------------------+
         | Snapshots + WAL  | Compaction + Tiering  |
         +------------------+-----------------------+
                               |
      Versioned DHIS2 Semantic Catalog / Lineage / Permissions
                               |
            Parser / Binder / Logical Planner / CBO
                               |
      Vectorized Physical Executor / Parallel Operators / MPP
                               |
        Incremental Materializations + Data Product Catalog
                               |
         Safe Analytics API + Results / Streaming / Export
                               |
        DER Visualiser | Reporting | Capture | Other Apps

 Side planes: Resource Governance, Recovery, Audit, Observability
 Edge-to-Server: secure federation, partial states and reconciliation
```

**Responsibility boundary:** transactional records/journal remain authoritative; analytical stores are rebuildable versioned derivatives; query serving must not bypass authentication and record-level access policy. Edge freshness is not the same as central acknowledgement.

### 2.1 Deployment profiles

| Profile | Deployment | Constraints |
|---|---|---|
| Embedded facility edge | DER Client library or managed local worker | Small resource budget, no cloud/control-plane dependency |
| High-volume hospital site | Dedicated site service | Multi-device ingest, local column projections and concurrency |
| Regional server | Several workers or vertically scaled service | Cross-site analysis, jobs and partial materializations |
| National / enterprise Server | Distributed worker cluster | MPP, node recovery, sharding, resource pools, optional storage/compute separation |
| Disaster recovery | Restorable standby | Snapshot, event-log replay and verifiable data lineage |

Server and Client share semantic AST, typed data model, format versions, aggregation-state codecs and API behavior; they need not have identical process topology. Edge must work without Kafka, Kubernetes, external quorum or S3.

### 2.2 Logical platform modules

`analytics-core`, `analytics-catalog`, `analytics-ingest`, `analytics-storage`, `analytics-runtime`, `analytics-planner`, `analytics-views`, `analytics-dhis2`, `analytics-distributed`, `analytics-serving`, `analytics-admin`, `analytics-benchmark`, `analytics-security`.

These are logical boundaries, **not mandated new directories**. Claude must audit the DER repository first and adapt interfaces and build systems to the existing architecture.

---

## 3. DER-owned analytical storage architecture

### 3.1 Data layout

The native storage fabric must support three distinct semantics: **append change log**, **latest-state versioned key table**, and **mergeable aggregate-state table**. The ingestion side should accept a bounded mutable delta/memtable; read-optimized facts live in immutable compressed column segments. Segment manifests atomically publish visible snapshots, with revision-aware tombstones and background compaction.

```text
analytics_root/
  catalog/                 # versioned schemas, dependencies, policies
  ingest_journal/          # durable analytical changes/checkpoints
  datasets/<id>/partitions/<id>/
    manifest               # committed live segment generation
    segments/<id>.derc      # proprietary DER columnar segment
    primary_index/         # stable source record IDs and versions
    secondary_indexes/     # selected page/bitmap/bloom structures
    delete_vectors/        # tombstones and superseded rows
  materialized_views/      # aggregate states, watermarks, lineage
  snapshots/               # validated restorable versions
  spill/                   # bounded, tracked temporary query files
  quarantine/              # malformed inputs and repair diagnostics
```

### 3.2 Native segment format `.derc`

Define and version a portable binary specification with: magic header; format major/minor; dataset/partition/segment IDs; schema and metadata fingerprints; row count; row groups; column metadata; typed pages; codec IDs; null/validity bitmap; dictionary metadata; zone maps; compressed buffers; payload offsets; per-page checksums; authentication/encryption metadata and optional feature flags. Persist manifests with an atomic commit protocol and validate before making a generation query-visible. Unknown critical features must fail closed rather than corrupt storage.

Test endian portability, integer/decimal precision, Unicode, nullable values, zero-row and wide-schema segments, corruption, torn write, future-version rejection and recovery. Readers hold a snapshot reference to prevent compaction deleting live data.

### 3.3 Encoding and compression

| Column characteristics | Candidate techniques | Acceptance criterion |
|---|---|---|
| Repeated categories and strings | Dictionary encoding | Low storage and fast predicate mapping |
| Ordered integers/periods | Delta or delta-of-delta | Lossless, measurable compression |
| Small integer ranges | Bit-pack / RLE | Faster vector decoding |
| Sparse/nullable fields | Bitmap or sparse representation | Distinguish NULL vs zero vs absent |
| Generic high-entropy payloads | LZ4/Zstd-equivalent reviewed codecs | Tunable compression/CPU trade-off |
| Timestamps | Delta-of-delta / packed blocks | Precision and timezone preservation |
| Exact decimal measures | Fixed decimal representation | No silent floating-point loss |

Choose codecs per-column/page via sampled statistics under bounded CPU overhead. Include codec fallback and on-disk version compatibility.

### 3.4 Indexes and pruning

Implement partition and column pruning; row-group/page min/max zone maps; sorted sparse indexes; Bloom filters for selective equality; bitmap indexes for low-cardinality categories; optional approved inverted indices; query-driven index selection. Statistics: rows, size, distinct estimates, null fraction, frequency sketches, min/max, sort order, compression ratios. Avoid indexing every field by default.

### 3.5 Key identity, updates and snapshots

Use stable DHIS2 record identifiers, not display labels. An aggregate value key includes data element, period, OU, category option combo and attribute option combo. Event, enrollment, tracked entity and relationship facts use their stable UID with source/revision context. Maintain idempotency key and monotonic source revision; apply version fences so out-of-order changes do not resurrect obsolete values. Readers see a consistent snapshot across delta, segments, delete vectors and manifests.

Support merge-on-write or a proven hybrid strategy selected by measured trade-offs; preserve fast scans while handling frequent Tracker edits and deletes. Correctness requirements include source correction, stale replay, late delete and metadata migration. Compaction cannot resurrect deleted/superseded rows.

### 3.6 Storage lifecycle

- Tiered hot/warm/cold placement (optional and deployment-specific), local disk always usable on edge.
- Compaction with budgets, cancellation, throttling and write-amplification metrics.
- Backups, encrypted snapshots, checksums, scrub/repair, WAL replay, selective rebuild.
- Disk-full/degraded mode that protects committed data and Capture availability.
- Governed retention and derived-data deletion, including cache/product invalidation.
- Schema migrations with feature gates, dry-run validation and rollback/restore procedures.

---

## 4. Semantic catalog and data model

The catalog stores versioned DHIS2 metadata: datasets/data elements/operands, category combos/options, attribute option combos, programs/stages/attributes, tracked entity types, option sets, organisation units and hierarchies, periods, program indicators and filters, indicator definitions, relationships, stage repeatability and access scopes. Every expression retains its original metadata revision, compiled typed AST, dependency graph and compatibility status.

**Typed analytic values:** Boolean, signed/unsigned integers, exact decimals, float, UTF-8, binary, date/time/timestamp, categorical/dictionary IDs, UID, optional geospatial, nullable arrays/structures. Coercions must be explicit and testable.

**Query consistency modes:** `EDGE_LATEST`, `EDGE_STABLE`, `CENTRAL_ACKNOWLEDGED`, and permitted `AS_OF` snapshots. Return applied watermark, metadata revision, site completeness and freshness/lag. Results must state whether they are exact, estimated or incomplete.

**Metric classes:** additive (sum/count with correction model), algebraic (avg from sum/count), nonadditive (distinct identities, ratios), holistic (median/exact percentile), temporal/cohort and relationship-derived. Never merge site-local distinct counts as if globally additive. Approximate sketches require explicit request and declared error bounds.

---
## 5. Native vectorized execution engine

### 5.1 Column batch runtime

The core runtime operates on typed, contiguous column batches with validity bitmaps, selection vectors, dictionary IDs, reference lifetimes and minimal copying. An Arrow-compatible adapter may be offered for interchange, but query execution is DER-controlled. Batch width adapts to memory, CPU cache, type width, filters and workload profile; maintain a scalar reference interpreter as the correctness oracle.

### 5.2 Required operators

| Family | Native operators |
|---|---|
| Scan | Full column scan, indexed scan, partition/segment/page pruning, delta+segment snapshot union |
| Relational | Filter, project, cast, sort, top-k, limit, union, distinct, conditional expression |
| Aggregation | Hash aggregate, sorted aggregate, partial/final states, grouping sets when supported |
| Join | Hash/broadcast/merge, semi/anti, bounded nested-loop fallback, runtime filters |
| Analytical | Window functions, time bucketing, cohort and distinct calculation, drilldowns |
| Distributed | Exchange, repartition, merge partial, retryable fragment, remote scan subject to policy |
| Runtime | Resource accounting, spill-to-disk, cancellation, cooperative scheduling and backpressure |

Support SIMD where available, with verified CPU-feature detection and portable fallback. Optional code generation/JIT must be isolated behind a feature flag after the vectorized engine is correct. Determinism matters more than blindly maximizing parallel threads.

### 5.3 Memory and query resource management

Every query has estimated and hard budgets for memory, CPU, scan bytes, temporary disk, output rows, execution duration and network transfer. External hash/sort and spill paths must work with limited RAM. Interactive queries have priority over large exports, rebuilds and compaction. Cancellation must clean temporary state without damaging committed storage.

### 5.4 Join strategies

Choose broadcast for genuinely small permitted dimensions; partitioned/shuffle joins for large datasets; merge joins for suitable sorted inputs. Detect skew and guard against cardinality explosion. Track per-fragment cardinality, build/probe size, spill and network bytes. Security rules override optimizer cost: no unauthorized patient-level data broadcast across sites.

---

## 6. Query language, planner and optimization

### 6.1 Query interfaces

1. Typed **DER Semantic Query DSL** (JSON) for supported data products, metrics, dimensions, filters, periods, drilldown and consistency.
2. Safe, documented **DER SQL subset** for `SELECT`, `WHERE`, `GROUP BY`, `HAVING`, joins, sorting, limits, CTEs and selected windows; extend deliberately.
3. DHIS2 metadata-driven query compiler for indicators/program indicators, aggregate, events, tracker and organisation hierarchy.
4. Saved query definitions, parameterized job templates and materialized product manifests.

All compile into the same logical AST and use the same authorization, cost model and executor. Unsupported features must fail with precise diagnostics rather than silent approximations.

### 6.2 Query lifecycle

```text
Authenticate -> Parse -> Bind metadata IDs/types -> Expand DHIS2 semantics
  -> Inject access policy -> Normalize logical AST -> Rewrite
  -> Estimate selectivity/cardinality/cost -> Choose physical plan
  -> Enforce quotas/admit -> Execute vector batches -> Annotate provenance
  -> Return result, cursor, job handle or streaming response
```

### 6.3 Optimizer capabilities

- Constant folding and expression normalization.
- Predicate pushdown and projection/column pruning.
- Partition, segment, page, index and runtime-filter pruning.
- Join reordering (only when semantically safe); join strategy selection.
- Aggregation pushdown, partial/final aggregation and late materialization.
- Materialized-view rewriting, cache reuse and plan fingerprints.
- Top-k pushdown, unnecessary sort avoidance and common subexpression elimination.
- Memory/network-aware fragment placement, limited dynamic replanning.
- Statistics feedback that is versioned, reversible and bounded.

**Cost inputs:** rows, bytes, histograms, null fractions, distinct estimates, skew, sort order, index selectivity, compression, cache warmth, disk throughput, CPU cost, memory and network locality. Explain plans should show alternatives rejected and key estimates when useful. `EXPLAIN ANALYZE` provides actual time, rows, scanned bytes, operator CPU, memory peak, spills, cache hits, network and per-fragment skew.

### 6.4 Optimizer correctness

Build property-based rewrite equivalence tests. Keep feature flags and a stable fallback physical plan. A bad estimate may make a query slower but cannot produce a different result. Bind cache keys to data revision, metadata revision, query semantics, policy context and requested consistency.

---

## 7. Streaming, CDC and continuous incremental analytics

### 7.1 Input pipelines

Ingest from DER's durable transactional journal/outbox, authorised PostgreSQL change streams, DHIS2 bulk/API imports, metadata updates, controlled historical backfill and optional typed external inputs. Normalize a canonical event envelope with source, site, object kind, stable ID, revision, action (UPSERT/DELETE), event/effective/ingest times, metadata version, idempotency key, hash, security classification and payload.

```json
{
  "changeId": "change-unique-id",
  "siteId": "site-uid",
  "objectType": "EVENT",
  "objectUid": "source-object-uid",
  "sourceVersion": 1042,
  "operation": "UPSERT",
  "occurredAt": "2026-11-11T07:00:00Z",
  "metadataRevision": "metadata-version-id",
  "payloadHash": "sha256:...",
  "payload": {}
}
```

This envelope is conceptual. Claude must map the existing DER journal and DHIS2 version contracts instead of creating incompatible surrogate IDs.

### 7.2 Ingestion guarantees

Durable acknowledgement, idempotent replay, version-aware deduplication, out-of-order buffering or fencing, checkpoint integrity, quarantine/error queues, restart recovery, batch/microbatch tuning, backpressure and ingestion-to-visible watermarks. Logical exactly-once effects may be built over at-least-once transport, provided tests prove it.

### 7.3 Incremental view maintenance

A dependency DAG links source facts/fields -> normalized projections -> indicators -> materialized states -> data products -> caches. On edit, the system must calculate **old and new contribution**; retract old and apply new where safe. Move event between periods and organisation units correctly. On delete, remove its contribution. For distinct and non-invertible computations, maintain exact reference states or selectively recompute only affected partitions. Approximate sketches cannot generally be decremented without a suitable design.

### 7.4 Late data and period engine

Implement event-time versus ingest-time, watermarks, allowed lateness, update windows, corrections to closed periods, explicit time zones and period boundaries. DHIS2 reporting frequencies (weekly variants, monthly, quarterly, yearly and supported relative/fiscal periods) must use verified version-specific semantics. Preserve original timestamps and event identity.

### 7.5 Data product freshness

Every materialization exposes last source watermark, last update time, source site coverage, metadata/compiler version, rebuild-needed state and central acknowledgement status. Consumers must be able to request strong or relaxed freshness and receive clear stale/partial warnings.

---

## 8. DHIS2-native analytics as a competitive advantage

### 8.1 Aggregate reporting

Support data elements, dataset associations, category combos, category options, category option combos, attribute option combos, organisation units, periods, indicators, reporting completion and quality rules. Preserve correct cell coordinates; do not substitute calculated totals for stored combinations. Aggregate missingness, zero and NULL have distinct meanings.

### 8.2 Event analytics

Filter/group by program, stage, event date, OU, event status, data elements, assigned user, options and declared dimensions. Handle record revisions, duplicates, deletions, repeatable stage events and conditional rule-generated values safely.

### 8.3 Tracker analytics

Link tracked entities, enrollments, events/stages and metadata by their correct identifiers. Support distinct tracked entity counts, repeat enrollments, longitudinal cohorts, stage completion and intervals, status histories, age/date logic, attributes, relationship direction and exact related events. No simplistic joins that duplicate numerators due to repeatable stages.

### 8.4 Program indicators and expression compatibility

Implement parser/typed compiler with a versioned function support matrix, semantic validation, numerator/denominator provenance, filtering, aggregation type, period behavior and option disaggregation. Tests compare supported cases to official DHIS2 output. Unsupported expressions are rejected or explicitly routed to a documented fallback; never silently approximate.

### 8.5 Analytical data products

Publish reusable: aggregate cubes, program-indicator tables, event fact views, tracker fact views, cohort datasets, service-point workload metrics, facility reporting, quality results, time series, geographical summaries, and permission-filtered line listings. Each product has a manifest: ID, owner, grain, dimensions/measures, permissions, source lineage, refresh mode, metadata/compiler version, watermark, quality checks, retention, compatibility version and declared consumer contract.

---

## 9. Distributed analytical fabric (Server + Client)

### 9.1 Modes

- **Site-local:** run entirely on an authorised site snapshot, independent of WAN.
- **Server-consolidated:** central acknowledged data and pre-aggregates.
- **Federated:** coordinator queries permitted sites using secure plan fragments.
- **Hybrid:** combine central acknowledged state with clearly marked fresher edge contributions only where correctness and permissions allow.

### 9.2 Distributed execution

Use a query coordinator, worker capability registry, partition assignment, fragment scheduler, bounded exchanges/shuffles, partial/final aggregation, spill, retry and cancellation. Prefer data-local computation. Record topology, worker capacity and site health. Large clusters may support separated compute and storage; small site must not depend on this infrastructure.

### 9.3 Distributed correctness

- SUM/COUNT may merge via version-aware idempotent contributions.
- AVG merges `sum,count` rather than averaging averages.
- MIN/MAX under deletions need sufficient state/recomputation.
- Distinct people across sites require identity-aware exact reconciliation or an explicitly approximate algorithm.
- Percentiles/medians require mergeable state or central recomputation with declared precision.
- Cross-site relationships must not be guessed from common names or demographics.
- Missing sites must yield a partial result or failure per contract, never a falsely complete national count.

### 9.4 Reconciliation after WAN interruption

The Client computes and saves locally, then replays signed/authorized changes or approved aggregation states. Server checks idempotency, record versions, metadata, permissions and site watermarks, applies corrections, acknowledges and returns reconciliation status. Conflicts feed an explicit DER resolution workflow. Central analytics are never allowed to invent a winning transactional value.

### 9.5 Privacy-first placement

Classify site data; configure whether raw, pseudonymized, aggregate-only or no data may leave site. Query planner checks privacy policy **before** sending fragments or selecting broadcast joins. Federated queries cannot use cost arguments to bypass residency and row-level authorization.

---

## 10. Serving APIs, interoperability and developer platform

Recommended versioned routes:

```text
GET  /api/analytics/v1/catalog
GET  /api/analytics/v1/data-products
POST /api/analytics/v1/query
POST /api/analytics/v1/query/explain
POST /api/analytics/v1/drilldown
POST /api/analytics/v1/jobs
GET  /api/analytics/v1/jobs/{id}
POST /api/analytics/v1/jobs/{id}/cancel
GET  /api/analytics/v1/indicators
GET  /api/analytics/v1/freshness
GET  /api/analytics/v1/engine/health
GET  /api/analytics/v1/exports/{id}
```

Define versioned OpenAPI/JSON schemas, typed errors, query limits, cursor pagination, timeouts, streaming output, cancellation, job polling, ETags, retries, result retention, data product discovery and compatible export formats such as CSV, JSON and Arrow IPC/Parquet when implemented. Use policies on every request and every async output release. Avoid unrestricted arbitrary SQL endpoints for general application users.

Provide a semantic SDK for DER apps so Visualiser/Reporting do not need to reconstruct indicator calculations. The platform should expose source lineage, freshness, query cost, metadata description and safe drilldowns to authorized source records.

---

## 11. Security and governance

Required: integrate DER identity/access controls; OU/program/record/field authorization where applicable; row/column security filters in query binding and execution; deny-by-default federation; site residency and sensitive-field classifications; encryption at rest/in transit; audit of queries/exports/admin actions; cache segregation by authorization context; secure token/key lifecycle; output size and small-cell disclosure controls; retention, backups and governed deletion across derived products.

No arbitrary network, filesystem or process execution via query language or user-defined functions. Defend against SQL/DSL injection, resource-exhaustion, malicious segments and metadata poisoning. Security tests must cover role revocation during pending jobs, cache cross-user leakage, explain-plan sensitive data and cross-facility joins.

---

## 12. Resource governance, performance and observability

Workload classes: interactive queries, dashboards, large exports, scheduled reporting, ingestion, incremental views, backfills, compaction, recovery and benchmarks. Each has queue priority, concurrency, memory/CPU/disk limits, pause/cancel and admission controls. Explicitly reserve transactional headroom on DER Client.

Expose p50/p95/p99 query latency, scanned bytes, useful rows/sec, row-group pruning, cache/materialization hit rates, ingestion-to-visible latency, stale view counts, CPU/memory/spill, write amplification, compaction debt, error/retry rate, dropped/quarantined inputs, site watermark lag and parity failures. Query profiles show chosen plan and actual costs while redacting protected fields.

### 12.1 Candidate performance targets (not existing measurements)

| Test | Initial aspirational engineering gate | Required benchmark context |
|---|---|---|
| Cached simple chart | p95 under 150 ms | Warm cache, local API roundtrip, fixed concurrency |
| Site materialized indicator | p95 under 500 ms | Local site benchmark and normal ingest |
| Interactive central aggregation | p95 under 2 seconds | Published 100M+ fact fixture and fixed hardware |
| Selective indexed line listing | p95 under 2 seconds | Bounded page, permission-enforced query |
| Local incremental freshness | p95 under 5 seconds | Source commit to visible site metric |
| Central incremental freshness | p95 under 30 seconds | Post-ack update, nominal resources |
| Exactness | 100% parity on supported deterministic fixtures | Reference/oracle output |
| Replay | No duplicate logical effects | Retry, crash, out-of-order tests |
| Capture isolation | Under 10% added Capture p95 latency | Same transactional workload with/without analytics |

Recalibrate targets after actual measurements. Never use a number without publishing hardware, data scale, concurrency, cache state, workload complexity and error bars.

### 12.2 Benchmark suite

Test 10K, 1M, 10M, 100M and 1B facts where hardware permits; aspirational 10B at server scale. Include skewed facility distribution, wide sparse event fields, high-cardinality IDs, frequent Tracker updates, late events, cohort joins, periods, categories and metadata version changes. Benchmark cold/warm, ingestion + queries, multiple users, low RAM, disk pressure, WAN outage, node failure and concurrent compaction.

Compare against pinned versions of ClickHouse and Doris **only as external reference benchmarks**, never runtime dependencies. Ensure equivalent hardware, data semantics, indexes/materializations, concurrency, cache state and update patterns. Publish scripts, raw outputs, accuracy results and regression plots. Success can be superior edge continuity, accuracy, operations, footprint or selected latency—not necessarily every SQL benchmark.

---
## 13. DER Analytics administration and operational UI

All screens inherit the DER Client full-screen app model: launch from the existing App Launcher, reuse the global application chrome, avoid duplicate navigation, use calm neutral surfaces, restrained accents, readable font sizes, compact tables and clear hierarchy. **Lists, details and edit forms must remain separate full-page routes.** Context drawers are for help, inspection and diagnostics, not embedding an editable form beside a working list. No gradients, decorative badges, unnecessary colored cards or unstable component sizing.

| Screen | Required display | Primary operations |
|---|---|---|
| Platform Overview | Ingest lag, site freshness, query p95, CPU/memory/disk, alerts | Drill into cause |
| Data Sources | Source type, checkpoint, failures, schema | Register, pause, resume, test, repair |
| Storage Catalog | Table/projection versions, segments, compression, partitions | Inspect, compact, rebuild, tier |
| Query Workbench | Query definition, EXPLAIN, actual profile, results | Run, compare, save, export |
| Query History | Query fingerprint, duration, scan bytes, spill, user scope | Inspect and rerun if authorized |
| Materialized Views | Dependencies, version, watermark, staleness | Refresh, rebuild, retire |
| Data Products | Manifest, measures, lineage, SLA, consumer list | Publish, version, deprecate |
| Distributed Nodes | Site/worker health, resources, fragments, availability | Drain, rebalance, repair |
| Reconciliation | Site journal watermarks, gaps, conflicts and central acknowledgment | Diagnose, retry approved operation |
| DHIS2 Parity | Version support and analytical result diffs | Inspect fixture, revalidate |
| Security & Audit | Access grants, queries, exports, policy revisions | Review and revoke |
| Backups & Recovery | Snapshots, log positions, restore checks | Validate and restore |
| Benchmarks | Historical comparisons and regressions | Run controlled test |
| Settings | Quotas, profiles, feature flags, retention | Validate, version and apply |

Loading, empty, offline, degraded, partial, permission-denied, stale, conflicting and failed states must be deliberately designed, with consistent typography and status language. No UI should claim 'synced' merely because it is locally durable.

---

## 14. Functional requirements traceability register

**Priority:** M = mandatory target platform capability; A = advanced after verified core. The table is an engineering target and does not assert implementation completeness.

| ID | Priority | Implementable requirement | Required verification |
|---|---|---|---|
| STO-001 | M | Native versioned DER column segment format | Codec/version roundtrip and corruption suite |
| STO-002 | M | WAL, atomic manifest, verified crash recovery | Kill/restart at commit boundaries |
| STO-003 | M | Typed pages, NULL masks, dictionaries and compression | Golden datasets and numerical parity |
| STO-004 | M | Snapshot-consistent delta/segment reads | Concurrent read/write tests |
| STO-005 | M | Stable primary key, revision fences, upsert/delete | Out-of-order and duplicate replay |
| STO-006 | M | Safe compaction and tombstone garbage collection | Reader/snapshot preservation |
| STO-007 | M | Partition/segment/page statistics and pruning | Explain scan-byte assertions |
| STO-008 | A | Adaptive indexes and optional storage tiering | Measured cost vs benefit |
| CAT-001 | M | Versioned DHIS2 semantic catalog | Metadata fixtures and migration |
| CAT-002 | M | Dependency and lineage graph | Impact analysis on field change |
| CAT-003 | M | Query consistency and provenance contracts | Watermark/partial response tests |
| ING-001 | M | Durable source adapters and canonical changes | Source-to-segment integration |
| ING-002 | M | Idempotent, ordered, checkpointed processing | Retry/replay/fault matrix |
| ING-003 | M | Bounded buffering, backfill and quarantine | High-rate ingestion and invalid input |
| RUN-001 | M | Typed vector batches and memory ownership | Scalar oracle equivalence |
| RUN-002 | M | Scan, filter, project, hash aggregate and sort | Query golden corpus |
| RUN-003 | M | Resource budgets, spill, cancel and fairness | Low-memory co-load test |
| RUN-004 | M | Parallel single-node execution | Deterministic concurrency suite |
| RUN-005 | A | Join algorithms, windows and runtime filters | Join oracle, spill and skew |
| OPT-001 | M | Logical rewrites and pruning | Property-based equivalence |
| OPT-002 | M | Cardinality/cost stats and EXPLAIN | Estimate/actual comparison |
| OPT-003 | A | CBO joins, materialized-view rewrite and adaptation | Comparative benchmark and rollback |
| INC-001 | M | Incremental dependency-based materialization | Old/new contribution tests |
| INC-002 | M | Period/OU correction and deletion retractions | Exact count assertions |
| INC-003 | M | Targeted recomputation for noninvertible states | Distinct/cohort tests |
| INC-004 | M | Freshness and invalidation metadata | Consumer lag/consistency tests |
| DHI-001 | M | Aggregate option combos/period/OU semantics | DHIS2 parity fixtures |
| DHI-002 | M | Event and Tracker fact models | Multiple enrollment/stage fixture |
| DHI-003 | M | Program indicator expression compiler | Versioned function parity suite |
| DHI-004 | M | Relationship and cohort measures | Controlled reference datasets |
| DST-001 | M | Autonomous site analytics without WAN | Offline reboot and analytics suite |
| DST-002 | M | Secure edge-server incremental reconciliation | Duplicate/reordered WAN replay |
| DST-003 | A | Distributed query fragments and execution | Multi-worker correctness and failures |
| DST-004 | A | Safe merging and data-residency policy | Cross-site distinct/privacy tests |
| API-001 | M | Typed, versioned secure analytical query API | Contract, load and authorization tests |
| API-002 | M | Data product, freshness and drilldown APIs | DER Visualiser/Reporting integration |
| API-003 | M | Async jobs, cancellation and bounded export | Large dataset and user revocation |
| SEC-001 | M | Row, column, OU, program, site policy enforcement | Cross-role/cross-site matrix |
| SEC-002 | M | Security-context-safe caching and diagnostic output | Leakage tests |
| SEC-003 | M | Encryption, governed deletion and export audit | Security/retention tests |
| OPS-001 | M | Metrics, tracing, profiling, query history | Operator diagnostic E2E |
| OPS-002 | M | Isolation from Capture and bounded background work | Capture latency co-load benchmark |
| OPS-003 | M | Snapshot restore and source-log rebuild | Disaster recovery drill |
| OPS-004 | M | Modern full-screen DER Admin app | UI E2E and accessibility tests |
| OPS-005 | M | Reproducible benchmarking and regression CI | Raw artifact generation |
| OPS-006 | A | Automated index/view/statistics tuning | Controlled A/B and safe rollback |

---

## 15. Claude implementation programme

Claude must inspect the current DER repository and existing Analytics implementation *before* introducing modules or rewriting services. Preserve compatibility with current DER Server, Client, Data Entry, Capture, metadata and sync journal. Each phase must yield functional code, integration evidence, failure testing and updated documentation. Do not represent a scaffold as a delivered capability.

| Gate | Focus | Exit evidence |
|---|---|---|
| 0 | Repo/source audit, gap matrix, ADRs, migration plan | Verified architecture inventory and requirement traceability |
| 1 | Semantics, types, metadata catalog, scalar oracle, query API slice | Correct end-to-end query over existing records |
| 2 | Native `.derc` storage, WAL, manifests, delta, revisions and compaction | Rebuild, crash and checksum tests |
| 3 | Vectorized scan, filters, grouping, sort, memory and spill | Oracle equality + benchmark baseline |
| 4 | Journal ingest, materialized views, retractions and freshness | Offline updates and correction convergence |
| 5 | Indexing, stats, CBO, joins, view rewrites, parallelism | Measured speedup without accuracy regression |
| 6 | Full targeted DHIS2 aggregate/event/tracker semantics | DHIS2 parity matrix and unsupported list |
| 7 | Federated/MPP server, cross-site consolidation and failures | Multi-node correctness, outage and privacy suite |
| 8 | Serving/data products, Admin app and diagnostics | Consumer and operator E2E journeys |
| 9 | Production qualification, performance comparison, security, migrations | Benchmark reports, recovery drills, rollback runbooks |

**Every gate deliverable:** change map, tests and results, reproducible benchmarks, API/schema updates, data migration notes, operational metrics, known limits, rollback and explicit pass/fail status. Unsupported DHIS2 functions, skipped tests and partially implemented workloads must be reported accurately.

### 15.1 Mandatory architectural decision records

Native format and migrations; storage key/version semantics; journal/manifest atomicity; query language; typed batch ownership; optimizer search space; DHIS2 indicator semantics; materialized-view correction model; secure federated protocol; site identity and access; edge resource limits; Server worker topology; result caching; encryption and retention.

### 15.2 Claude kickoff instruction

> Implement `DER_Analytics_Platform_Architecture_and_Feature_Specification.md` together with the existing DER Analytics SRS. Audit the current DER Server and Client repository, services, metadata models and journal first. Build the native DER-owned column segment format, version-aware storage, vectorized query executor, statistics-driven optimizer, incremental materializations, DHIS2 semantic compiler, edge-server reconciliation, distributed execution, secure consumer APIs and DER Admin workflows. Do not install or depend on ClickHouse, Doris, DuckDB or another independently operated OLAP database. Implement progressively using the gate exit requirements, correctness oracles, fault injection, DHIS2 parity and published benchmark results. Preserve the existing DER design system and all transactional workflows; identify incomplete features precisely rather than claiming them complete.

---

## 16. Comprehensive end-to-end acceptance scenarios

1. Aggregate value changes category option combo: old contribution retracts, new combination updates, no double-count.
2. Tracker event date moves to an earlier reporting period: old period decreases and new one increases correctly.
3. Same ingest change delivered ten times and out of order: single logical effect and correct latest version.
4. Tracked entity with two enrollments and repeatable stage: distinct people and event totals are separately correct.
5. Event and enrollment joins preserve multiplicity and program-stage filters.
6. Relationship type/endpoint changes invalidate and rebuild affected relationship/cohort data products.
7. Site collects offline for a full day: local queries continue and are labeled as site-only.
8. Reconnect transmits pending deltas: central output converges and duplicate retries do not distort it.
9. Two devices update disjoint authorized fields: edge projection reflects correct reconciled value versions.
10. Concurrent conflicting updates are not silently counted twice; analytics follows resolved transactional version.
11. Metadata update changes a program indicator definition: products show stale state until appropriate rebuild.
12. Central query encounters unavailable site: returns explicit partial site coverage or fails per contract.
13. Distinct patients across sites cannot be summed from per-site distinct totals without approved identity strategy.
14. Query scans only selected period and data-element columns; explain reflects actual pruning.
15. Query on a high-cardinality join spills safely and respects memory limits without wrong results.
16. Cached result cannot leak across users with different OU/field-level permissions.
17. User access revoked during a long export: final result release rechecks authorization.
18. Disk fills during segment compaction: committed data remains readable after restart.
19. Crash between log append and manifest commit: replay recovers without acknowledged logical loss.
20. Reader continues on consistent snapshot during concurrent ingest/compaction.
21. Older segment format opens through tested compatible reader or triggers safe migration/failure.
22. Program indicator fixtures match verified DHIS2 reference results for supported functions.
23. Invalid/unsupported expression fails clearly, without silently approximating semantics.
24. Worker failure during distributed query produces bounded retry or correct failure status.
25. Server can restore from snapshot plus journal and reproduce materialized values and checksums.
26. DER Client runs without external coordinators, object stores or internet access.
27. SQL/DSL cannot execute arbitrary system commands or bypass row-level security.
28. Operator can identify slow query, inspect scanned bytes and plan, and measure corrective improvement.
29. Analytics under full load respects the defined Capture/Data Entry p95 protection threshold.
30. A pre-registered benchmark reproduces a real DER advantage without reduced correctness.

---

## 17. Quality and testing strategy

**Unit/property-based:** codecs, type conversion, nulls, precision, primary-key revisions, optimizer equivalence, aggregate-state merge rules, time buckets. **Fuzz:** binary reader, DSL/SQL parser, expression compiler, metadata ingest. **Differential:** scalar oracle, PostgreSQL/reference SQL when comparable, official DHIS2 analytics and indicator outputs. **Integration:** source journal -> segment -> materialized view -> API -> Visualiser. **Chaos:** torn writes, disk full, process kill, stale CDC, WAN split, worker loss, clock skew, schema changes. **Security:** role matrix, policy revocation, export audit, data residency. **Load:** ingestion with read concurrency, compaction under Capture workloads, cold/warm queries, 100M+ fact fixtures, skew and low-RAM profiles.

Release-critical tests for confidentiality, durability, correctness and recovery cannot be waived because a performance score improves. Publish test seeds, generator configurations, datasets (synthetic or anonymized), query corpus, hardware, software versions, raw outputs and known discrepancies.

---

## 18. Risks and engineering safeguards

| Risk | Mitigation |
|---|---|
| Reinventing an entire DBMS without enough resources | Strict vertical slices, staged gates, leverage audited low-level libraries |
| Write/version bugs cause incorrect indicators | Correctness oracle, WAL, version fences, old/new contribution tests |
| Poor optimizer choices | Conservative fallback, query hints only for diagnostics, flag rollback |
| Excess indexing and compaction harms ingest | Adaptive budgets, write amplification and CPU limits |
| Multi-site patients are not globally identifiable | Explicit identity/dedup policy, never naive distinct merging |
| Metadata revisions change meaning | Catalog version history, dependency invalidation and targeted rebuild |
| Distributed networking exposes restricted data | Residency-aware planner and authenticated fragment execution |
| Excess analytics loads impact Capture | Reserved resources, quotas, priorities and co-load benchmarks |
| Misleading ClickHouse/Doris performance claims | Fair reproducible benchmark matrix with pinned versions |
| Custom storage corrupts after failure | Atomic manifests, checksums, crash injection, restore drills |

---

## 19. Documentation deliverables

Architecture diagrams; storage binary format spec; schema and index contracts; query DSL and SQL subset grammar; semantic function/period support matrix; DHIS2 metadata compatibility matrix; typed API schema; security/threat model; retention and incident response; ADRs; deployment modes; operations and recovery runbooks; performance tuning guide; query explain/profile guide; synthetic workload generator and reproducible benchmark suite; migration and rollback strategy; functional requirement traceability matrix.

---

## 20. Reference architecture sources

The following references inform design comparisons only; they are **not required DER runtime dependencies**.

1. ClickHouse architecture and MergeTree concepts: https://github.com/ClickHouse/docs-reference/blob/master/docs/en/development/architecture.md
2. Apache Doris system architecture, compute and storage: https://doris.apache.org/docs/4.x/features-architecture/system-architecture/
3. Apache Doris Unique Key / merge-on-write: https://doris.apache.org/docs/4.x/key-features/unique-key/
4. Apache Doris data model: https://doris.apache.org/docs/4.x/key-features/data-model/
5. Apache DataFusion query execution and Arrow concepts: https://datafusion.apache.org/user-guide/introduction.html
6. Apache Arrow column batch overview: https://datafusion.apache.org/user-guide/arrow-introduction.html
7. DHIS2 documentation and metadata/analytics reference: https://docs.dhis2.org/

**Document control:** This is a target platform specification, not verified implementation status. Final behavior, supported DHIS2 expressions, runtime resource thresholds and performance metrics require repository inspection, versioned compatibility verification and actual testing before release.
