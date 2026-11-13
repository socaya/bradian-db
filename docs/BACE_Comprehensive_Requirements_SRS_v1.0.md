# BRADIAN DATABASE SYSTEMS

BRADIAN ADAPTIVE COMPRESSION ENGINE (BACE)

Comprehensive System Requirements Specification, Architecture Contracts, Performance Assurance and Implementation Programme

Document ID: BDS-BACE-SRS-001 | Revision: 1.0 | Date: 13 November 2026 | Owner: Vitalworks Limited | Status: Proposed engineering baseline — not an assertion of completed implementation

Basis: Bradian Master Product Architecture & SRS v1.1 (BDS-MASTER-001, 12 November 2026); repository inspection of socaya/bradian-db main on 13 November 2026. All new BACE-specific designs and test thresholds in this document are recommendations pending architectural approval and benchmarking.

## Executive specification

BACE is the unified, lossless-by-default, workload-aware compression capability for the complete Bradian Database Systems platform. It governs the physical representation and movement of data across relational transactions, analytical column segments, hybrid projections, indexes, MVCC history, WAL and checkpoints, intermediate query spill, streaming, replication, federation, backups and supported import/export pathways. Its central objective is to minimize physical bytes while preserving exact database semantics, high throughput, predictable latency and safe recovery.

Compression is not a universal instruction to encode all bytes. BACE shall offer a fast raw-path fallback when compression is larger or more expensive than justified by the workload policy. No practical lossless compressor can simultaneously guarantee globally minimum space and zero CPU overhead for all possible inputs. Consequently, Bradian shall optimize the measured total cost of ownership under explicit latency, CPU, memory and integrity constraints rather than claim an impossible universal guarantee.

## 1. Document purpose, authority and scope

### 1.1 Purpose and normative interpretation

This specification defines capabilities, interfaces, policy decisions, invariants, threat controls, measurable non-functional criteria and release evidence for BACE. MUST means mandatory for the adopted BACE target; SHOULD is a default with a documented exception; MAY is an optional extension. A requirement is complete only when implementation, backward-compatibility, fault handling and acceptance tests are evidenced. No implementation claim follows merely from the existence of a module or placeholder.

### 1.2 Alignment with the master architecture

The Bradian master architecture defines a general-purpose relational database with native OLTP, OLAP and coherent HTAP, a single authoritative transaction history, committed columnar derivatives, distributed shards and replicas, autonomous federation, secure connectors, and Bradian Studio. BACE must operate within these contracts. In particular, a compression rewrite may not alter commit order, MVCC visibility, schema versions, tenant authorization, index semantics or the strong-snapshot watermark of a hybrid query.

### 1.3 Included and excluded boundaries

Included: durable pages, values, dictionaries and manifests; row and column representation; snapshot and history lifecycle; indexes and metadata; WAL and checkpoints; network transfer; backup/PITR; analytical working sets, spill and caches; replication and federation; and administrative configuration/telemetry. Compression of ephemeral CPU registers is not a product requirement. Lossy alteration of SQL results, implicit decimal rounding, floating-point value changes, weakening of audit evidence or cryptographic transformations marketed as compression are prohibited. Encryption is a separate, mandatory control and normally follows compression on the chosen payload.

## 2. Goals, constraints and measurable success

The primary storage metric is end-to-end physical consumption, not just compressed column data. A complete measurement must count authoritative pages, indices, retained MVCC history, delta structures, WAL, analytical projections, dictionaries, manifests, background-compaction overlap, temporary spill, audit material and backup copies when included in the stated scope. Storage amplification shall distinguish intentional redundancy required for durability or replication from avoidable duplication.

**BACE-GOAL-001 — Minimum practical physical footprint.** BACE MUST measure and choose efficient storage representations within a declared performance envelope; it shall not inflate already incompressible data beyond a configurable small framing allowance. **Verification:** Test synthetic, real and adversarial data distributions and compare disk bytes including codec metadata.

**BACE-GOAL-002 — Performance-first operation.** Compression MUST preserve the latency and throughput budgets applicable to each workload class, including OLTP commits, point reads, scans, mixed HTAP, replication catch-up and recovery. **Verification:** Compare p50/p95/p99, throughput and CPU on pinned hardware with compression enabled and baseline profiles.

**BACE-GOAL-003 — Exact semantics.** Primary stored data MUST be losslessly recoverable including NULL distinction, exact DECIMAL scale/value, signed zero, NaN payload where binary-preservation is required, timestamps, collation-relevant bytes and raw binary values. **Verification:** Type-specific property tests and reference equality across every supported codec.

**BACE-GOAL-004 — Universal coverage with selective execution.** Every Bradian storage/transport subsystem MUST expose a documented compression policy, including intentional RAW selection for hot or incompressible payloads. **Verification:** Component coverage matrix and zero-unowned-storage audit.

## 3. Architecture and execution contracts

The logical design comprises: (a) a common compression policy and capability registry; (b) typed statistics/profiling and deterministic candidate selection; (c) specialized storage adapters for BRPG relational pages, .derc columns, WAL and binary exchange; (d) versioned codec metadata and bounded decoders; (e) background recompression and compaction orchestration; and (f) observability and acceptance evidence. Common policy does not imply a single wire format, allocator, page layout or transaction boundary across all components.

Pipeline: logical values → type-aware statistics → eligible inner encodings (constant, dictionary, RLE, delta, FOR, bit-packed, specialized floating point) → optional outer compression (LZ4/ZSTD/raw) → versioned authenticated or checksummed envelope → durable or transport-specific publication. Reads reverse the pipeline under size limits and should preferentially apply encoded-domain filtering, predicate pushdown, dictionary comparisons and SIMD decoding when proven correct.

### 3.1 Integration boundaries

**BACE-ARCH-001 — Central policy service.** A versioned policy model MUST resolve global, workload, database, table, column, tier and job-level options with explicit precedence. Security and format capabilities override requested codec choices. **Verification:** Policy inheritance, conflict and downgrade tests.

**BACE-ARCH-002 — Codec registry.** Each codec MUST publish stable identifier, format version, supported types, parameters, min/max sizes, decoder bounds, determinism and SIMD availability. Unknown critical codecs MUST fail closed. **Verification:** Capability negotiation and unsupported-codec fixtures.

**BACE-ARCH-003 — No dual authority.** BACE MUST treat relational committed state and commit log as authoritative; column projections and compressed materializations remain derivatives with publication watermarks. **Verification:** Cross-store snapshot and lag/fallback tests.

**BACE-ARCH-004 — Isolation of compression work.** Heavy candidate evaluation and recompression MUST use dedicated resource-governed jobs rather than extend lock-holding or critical commit phases. **Verification:** Mixed-load admission and lock-duration measurements.

**BACE-ARCH-005 — Versioned compatibility.** On-disk/wire compression metadata MUST be self-describing enough for bounded decoding and independent of host endian/layout, without mutating pre-existing codec ID semantics. **Verification:** Golden files on Linux/macOS/Windows and forward/reverse compatibility matrix.

**BACE-ARCH-006 — Encryption ordering.** Compression MUST precede authenticated encryption of the payload where permitted, while headers and algorithm identifiers are integrity protected according to the security design. **Verification:** Tampering, wrong-key and codec metadata authentication tests.

## 4. Functional requirements by subsystem

### 4.1 OLTP row store and page cache

Scope: OLTP row store and page cache. The subsystem adapter shall inherit the BACE policy and must document its own safety-critical publication and read contracts; common compression utilities do not replace existing durability or authorization checks.

**BACE-FR-ROW-001 — Hot row encoding.** Hot rows MUST favor latency-predictable encodings, optional value-level compression for large varlen values, and a raw fallback. Compressed pages must not require full-table decompression for a point lookup. **Verification:** P95 random lookup and UPDATE benchmarks with mixed value sizes.

**BACE-FR-ROW-002 — Page compatibility.** The BRPG envelope, page IDs, checksums, free-space handling and B+tree addresses MUST remain valid. A new page representation must use negotiated feature/version metadata and safe migration. **Verification:** Legacy page fixtures, restart and power-loss tests.

**BACE-FR-ROW-003 — Overflow values.** Large strings, JSON and binary values SHOULD use individually framed chunks to enable selective reads and avoid rewriting unrelated row fields. **Verification:** Partial-read, update amplification and chunk corruption tests.

**BACE-FR-ROW-004 — Buffer pool.** Cache accounting MUST count resident decoded and compressed buffers; eviction policy MAY retain compressed copies when beneficial without duplicating unbounded memory. **Verification:** Memory cap and cache hit/miss workload tests.

### 4.2 Columnar OLAP and .derc segments

Scope: Columnar OLAP and .derc segments. The subsystem adapter shall inherit the BACE policy and must document its own safety-critical publication and read contracts; common compression utilities do not replace existing durability or authorization checks.

**BACE-FR-COL-001 — Type-aware pages.** Pages MUST support type-specific lossless encodings and optional outer codecs; selection shall be made per bounded page or homogeneous run. **Verification:** Round-trip and scan tests for every logical type.

**BACE-FR-COL-002 — Compact dictionary.** Dictionary IDs SHOULD use the minimum viable width; global/shared dictionaries require immutable identity, lifecycle and concurrency contracts. **Verification:** Cardinality boundary, reference and compaction tests.

**BACE-FR-COL-003 — Page granularity.** The writer SHOULD support multiple row groups and pages with fixed maximum uncompressed size, selective decompression and zone maps. **Verification:** Pruning, memory and partial scan benchmarks.

**BACE-FR-COL-004 — Encoding-aware execution.** The query engine SHOULD evaluate eligible filters/aggregates on dictionary, constant or RLE encoded data rather than blindly materialize every value. **Verification:** Differential query oracle and SIMD/scalar parity.

**BACE-FR-COL-005 — Column statistics.** Per-page statistics MUST include physical bytes, codec, null count, cardinality estimate, min/max where valid, decode cost and compression savings. **Verification:** EXPLAIN/telemetry agreement with physical pages.

### 4.3 HTAP projections and materializations

Scope: HTAP projections and materializations. The subsystem adapter shall inherit the BACE policy and must document its own safety-critical publication and read contracts; common compression utilities do not replace existing durability or authorization checks.

**BACE-FR-HTAP-001 — Commit-coordinate preservation.** Compression and recompression MUST preserve source commit coordinates, projection watermarks, visibility barriers and revision fencing. **Verification:** Concurrent OLTP/OLAP same-snapshot query suites.

**BACE-FR-HTAP-002 — Online projection rewrite.** Reencoding a projection MUST publish a new consistent manifest generation, keep pinned old generations and safely roll back on failure. **Verification:** Crash during rewrite; live-reader consistency.

**BACE-FR-HTAP-003 — Projection deduplication.** Derived projection sharing MAY eliminate duplicate physical payloads only when reference ownership, security domains and snapshot lifetimes are explicit. **Verification:** Tenant isolation, delete and snapshot pin tests.

### 4.4 Indexes, search and metadata

Scope: Indexes, search and metadata. The subsystem adapter shall inherit the BACE policy and must document its own safety-critical publication and read contracts; common compression utilities do not replace existing durability or authorization checks.

**BACE-FR-IDX-001 — Index representation.** B+tree keys SHOULD use safe prefix/delta encoding within independently readable pages; postings and integer offsets MAY use FOR/bit-packing. **Verification:** Range/point correctness and split/rebalance crash tests.

**BACE-FR-IDX-002 — Searchable path.** Hot index lookup MUST not depend on decompressing unrelated data pages or entire index levels. **Verification:** Point/range index p95 latency.

**BACE-FR-IDX-003 — Metadata and catalogs.** Repeated catalog strings and schema descriptors MAY use compact dictionaries, but schema version identity and atomic DDL semantics MUST remain unchanged. **Verification:** DDL replay, schema migration and catalog bootstrap.

### 4.5 WAL, checkpoints and MVCC

Scope: WAL, checkpoints and MVCC. The subsystem adapter shall inherit the BACE policy and must document its own safety-critical publication and read contracts; common compression utilities do not replace existing durability or authorization checks.

**BACE-FR-DUR-001 — WAL latency.** WAL compression MUST use bounded fast codecs or raw records, protect record framing and sequence/CRC rules, and never defer durable acknowledgment improperly. **Verification:** Commit p99, torn/truncated log and recovery tests.

**BACE-FR-DUR-002 — History compression.** Old MVCC versions SHOULD support shared-prefix or delta representation where chains are bounded and independently recoverable from verified anchors. **Verification:** AS OF reads, retention, rollback and version GC.

**BACE-FR-DUR-003 — Checkpoint processing.** Checkpoint compression MUST preserve page-LSN and index address consistency; compression work SHOULD be paced outside foreground commit paths. **Verification:** Crash, disk-full and repeated checkpoint trials.

**BACE-FR-DUR-004 — Recovery availability.** Recovery MUST fail safely for unknown codecs, oversized output and corrupted compressed frames without accepting uncommitted state. **Verification:** Fuzz and power-loss recovery matrix.

### 4.6 Distributed replication and federation

Scope: Distributed replication and federation. The subsystem adapter shall inherit the BACE policy and must document its own safety-critical publication and read contracts; common compression utilities do not replace existing durability or authorization checks.

**BACE-FR-NET-001 — Negotiated transport.** Peers MUST negotiate codec/version/maximum frame size and fall back safely to common encodings without silent semantic downgrade. **Verification:** Mixed-version upgrade and downgrade trials.

**BACE-FR-NET-002 — Replicated log identity.** Compressing a replicated batch MUST retain logical record identity, ordering, quorum semantics, idempotency and verifiable end-to-end integrity. **Verification:** Failover, partition and retry tests.

**BACE-FR-NET-003 — Batch and WAN policy.** Replication and federation SHOULD adapt compression to link bandwidth/latency and CPU headroom, avoiding compression on tiny or incompressible packets. **Verification:** High/low bandwidth and loss/RTT benchmarks.

**BACE-FR-NET-004 — Remote privacy.** Cross-tenant dictionary reuse and content deduplication MUST NOT leak protected equality or data across authorization or encryption domains. **Verification:** Tenant and federation isolation tests.

### 4.7 Streaming, ingestion, SQL spill and memory

Scope: Streaming, ingestion, SQL spill and memory. The subsystem adapter shall inherit the BACE policy and must document its own safety-critical publication and read contracts; common compression utilities do not replace existing durability or authorization checks.

**BACE-FR-RUN-001 — Ingestion streaming.** Streaming batches SHOULD be compressed after validation/staging under backpressure and replay checkpoints; invalid records must remain quarantined. **Verification:** Interrupted ingestion and exactly-once logical effects.

**BACE-FR-RUN-002 — Spill efficiency.** Sort/join/aggregate spills MAY use fast compressed chunks with checksummed headers and explicit IO/CPU budgets; cancellation MUST reclaim them. **Verification:** Large joins, quota and cancellation suites.

**BACE-FR-RUN-003 — Intermediate memory.** In-memory compressed vectors MAY be retained only where improved memory pressure outweighs decoding overhead and allocations are bounded. **Verification:** Memory peak, latency and allocator pressure.

**BACE-FR-RUN-004 — Import/export compatibility.** Arrow/Parquet/CSV/JSON adaptors MUST preserve source values and format contracts; BACE cannot silently rewrite externally specified compression semantics. **Verification:** Interchange round-trips against independent readers.

### 4.8 Backups, recovery and lifecycle

Scope: Backups, recovery and lifecycle. The subsystem adapter shall inherit the BACE policy and must document its own safety-critical publication and read contracts; common compression utilities do not replace existing durability or authorization checks.

**BACE-FR-BKP-001 — Backup efficiency.** Full/incremental/PITR archives SHOULD use strong streaming compression or source-page reuse, with independent bounded restore and authenticated manifest verification. **Verification:** Backup size, restore time and bit-exact verification.

**BACE-FR-BKP-002 — Incremental references.** Deduplication across snapshots MAY be used only with explicit reference manifests, retention pinning, encryption domain and disaster recovery guarantees. **Verification:** Delete/expire and missing-chunk restore trials.

**BACE-FR-BKP-003 — Recompression lifecycle.** Recompression MUST publish copy-on-write or generation-safe output, never replace an authoritative file before durable verification, and reclaim obsolete bytes only after all pins expire. **Verification:** Crash injection at every state transition.

**BACE-FR-BKP-004 — Audit and security evidence.** Compression MUST not weaken audit append-only sequencing, tamper-evidence, protected retention or authorization checks. **Verification:** Audit hash-chain/sequence and retention checks.

## 5. Codec catalogue and selection specification

BACE shall distinguish the inner type encoding from any outer byte-stream compression. Codec selection is not based solely on a fixed dictionary-cardinality threshold. It must consider null fraction, entropy proxy, distinct count, min/max span, run distribution, sortedness, value width, payload size, fixed-versus-varlen type, dictionary cost and the chosen workload policy. A raw candidate always participates in selection.

**BACE-CODEC-001 — Constant / null-only.** The codec registry SHOULD expose Constant / null-only as an eligible candidate for uniform or all-null runs, when validated and compatible with the page format. Minimal metadata and direct constant scans. **Verification:** Golden vectors, corruption fixtures, cross-platform deterministic decode and comparative size/speed evidence.

**BACE-CODEC-002 — RLE.** The codec registry SHOULD expose RLE as an eligible candidate for long consecutive repetitions, when validated and compatible with the page format. Run-aware filtering, bounded run lengths. **Verification:** Golden vectors, corruption fixtures, cross-platform deterministic decode and comparative size/speed evidence.

**BACE-CODEC-003 — Dictionary + packed IDs.** The codec registry SHOULD expose Dictionary + packed IDs as an eligible candidate for repeated text or categories, when validated and compatible with the page format. Cardinality and dictionary overhead aware. **Verification:** Golden vectors, corruption fixtures, cross-platform deterministic decode and comparative size/speed evidence.

**BACE-CODEC-004 — FOR / patched FOR.** The codec registry SHOULD expose FOR / patched FOR as an eligible candidate for small-range integer values, when validated and compatible with the page format. Overflow/exception path explicitly encoded. **Verification:** Golden vectors, corruption fixtures, cross-platform deterministic decode and comparative size/speed evidence.

**BACE-CODEC-005 — Delta / delta-of-delta.** The codec registry SHOULD expose Delta / delta-of-delta as an eligible candidate for ordered ids and timestamps, when validated and compatible with the page format. Signed arithmetic bounds and zigzag correctness. **Verification:** Golden vectors, corruption fixtures, cross-platform deterministic decode and comparative size/speed evidence.

**BACE-CODEC-006 — Bit-packed / Boolean bitmap.** The codec registry SHOULD expose Bit-packed / Boolean bitmap as an eligible candidate for narrow integer and boolean domains, when validated and compatible with the page format. SIMD/scalar parity and tail-bit validation. **Verification:** Golden vectors, corruption fixtures, cross-platform deterministic decode and comparative size/speed evidence.

**BACE-CODEC-007 — FSST or equivalent.** The codec registry SHOULD expose FSST or equivalent as an eligible candidate for short strings with repeated substrings, when validated and compatible with the page format. Trusted library, dictionary/escape format. **Verification:** Golden vectors, corruption fixtures, cross-platform deterministic decode and comparative size/speed evidence.

**BACE-CODEC-008 — Gorilla / Chimp / ALP candidate.** The codec registry SHOULD expose Gorilla / Chimp / ALP candidate as an eligible candidate for floating-point distributions, when validated and compatible with the page format. Bit-exact floating preservation and exception path. **Verification:** Golden vectors, corruption fixtures, cross-platform deterministic decode and comparative size/speed evidence.

**BACE-CODEC-009 — LZ4.** The codec registry SHOULD expose LZ4 as an eligible candidate for hot, throughput-oriented pages/batches, when validated and compatible with the page format. Avoid expansion; fast decode. **Verification:** Golden vectors, corruption fixtures, cross-platform deterministic decode and comparative size/speed evidence.

**BACE-CODEC-010 — ZSTD.** The codec registry SHOULD expose ZSTD as an eligible candidate for cold pages, backups, repetitive payloads, when validated and compatible with the page format. Configurable level with explicit CPU budget. **Verification:** Golden vectors, corruption fixtures, cross-platform deterministic decode and comparative size/speed evidence.

**BACE-CODEC-011 — RAW.** The codec registry SHOULD expose RAW as an eligible candidate for high-entropy or latency-critical inputs, when validated and compatible with the page format. Zero decoding and limited framing overhead. **Verification:** Golden vectors, corruption fixtures, cross-platform deterministic decode and comparative size/speed evidence.

### 5.1 Adaptive cost model

For each eligible candidate, calculate total_stored_bytes = payload + validity + dictionary + codec parameters + per-page framing + alignment + checksums/tags. Estimate decode CPU, encode CPU, resident memory, read amplification and expected I/O savings using observed hardware metrics. Reject candidates violating hard limits before ranking. In the balanced policy, minimize weighted total read/write/space cost; in maximum-space policy, minimize bytes only among candidates meeting latency and resource ceilings. Do not repeatedly try expensive candidates on the foreground commit path.

**BACE-SEL-001 — Deterministic decision evidence.** The selector MUST record why a codec was accepted or rejected, including measured or sampled byte savings and constraints, without logging sensitive data values. **Verification:** Repeatability tests and sampled decision audit.

**BACE-SEL-002 — Avoid expansion.** The selector MUST choose RAW or a cheaper option if encoded bytes plus framing are larger than the tolerated limit; small pages may be skipped based on measured threshold. **Verification:** Random/high-entropy and tiny-payload corpus.

**BACE-SEL-003 — Sampling and bound.** Sampled codec trials MUST have bounded CPU, memory and elapsed work; deterministic fallback applies if the budget is exhausted. **Verification:** Resource governor and adversarial input tests.

**BACE-SEL-004 — Workload-aware profiling.** FAST, BALANCED and MAXIMUM profiles MUST map to explicit measurable budget settings, not hardcoded promises of compression ratios. **Verification:** Configuration validation and profile benchmark matrix.

## 6. Persistent format, integrity and recovery requirements

Existing .derc v1 identifies codecs 0 Plain, 1 PlainUtf8, 2 Dictionary and reserves 3 RLE, 4 Delta, 5 BitPacked. The current writer is limited to the first three; ZSTD level 3 wraps pages by default when enabled. New codecs must not repurpose reserved numeric meanings incompatibly; feature flags and page metadata need a compatible reader strategy. The relational BRPG page and BRWL log are distinct formats and cannot be migrated implicitly as a side effect of .derc upgrades.

**BACE-FMT-001 — Strict envelope.** Every compressed durable object MUST carry validated version, codec ID, flags, encoded length, decoded length, bounds and integrity material; dictionary references must resolve within authorized immutable scope. **Verification:** Malformed length, overflow and missing-dictionary fuzz corpus.

**BACE-FMT-002 — Atomic publication.** Compression write paths MUST use durable staging, integrity verification, atomic manifest/generation publication and snapshot-safe cleanup where the storage backend supports the required guarantees. **Verification:** Injected crash at write, fsync, rename, manifest and GC stages.

**BACE-FMT-003 — Bounded decompression.** Readers MUST reject declared decoded sizes above configured page/frame maximum before allocating and enforce bounded output during decoding, not merely after decode completion. **Verification:** Decompression bomb and malicious frame tests.

**BACE-FMT-004 — Rollback and rollback-safe upgrades.** Upgrades MUST preserve legacy readers for existing files or require a formally verified, backed-up migration with explicit downgrade restriction. **Verification:** Rollback simulation and multi-version mixed file catalog.

**BACE-FMT-005 — Checksums versus authenticity.** CRC checks detect accidental corruption only; encrypted or untrusted distributed/backup payloads MUST use authenticated integrity and approved cryptographic libraries. **Verification:** Active tampering and checksum-only attack demonstrations.

## 7. Non-functional requirements and performance gates

The following thresholds are proposed acceptance gates, not empirical claims. A baseline must be measured on pinned CPU/storage, data corpus, dataset distribution, cache warmness, concurrency, isolation level and durability mode. “No performance impact” is not a scientifically achievable blanket guarantee; Bradian must instead enforce contract-specific regression budgets and bypass compression when needed.

**BACE-PERF-001 — Foreground commit p95 and p99.** Proposed gate: ≤5% p95 and ≤10% p99 regression against matched RAW baseline under the declared OLTP fixture; no loss of durable acknowledgment. **Verification:** Benchmark/test: Mixed short/long transactions; concurrent writers.

**BACE-PERF-002 — Point lookup.** Proposed gate: ≤5% p95 regression for cache-hot indexed point reads under the fixed key-value corpus. **Verification:** Benchmark/test: Warm-cache random read, 1–64 KiB values.

**BACE-PERF-003 — Analytical scans.** Compression SHOULD improve or preserve p95 scan time; any slower path beyond 5% on a declared query class must trigger a codec/policy review. **Verification:** Benchmark/test: Column scans, filters, aggregate and joins.

**BACE-PERF-004 — Ingestion.** Proposed gate: ≤10% sustained ingestion throughput regression in BALANCED; FAST must be benchmarked separately. **Verification:** Benchmark/test: Bulk and streaming loads; backpressure.

**BACE-PERF-005 — Resource isolation.** Background compression MUST remain within CPU/IO/memory budgets and must not exceed the master architecture mixed-workload OLTP p95 regression envelope. **Verification:** Benchmark/test: Concurrent OLTP/OLAP/recompress/backup stress.

**BACE-PERF-006 — Read amplification.** Hot selective reads MUST avoid decompressing unrelated chunks; record bytes-read and bytes-decoded per query. **Verification:** Benchmark/test: Small-field predicates on large objects.

**BACE-PERF-007 — Compression effectiveness.** Report physical bytes, user payload savings, per-codec savings, write amplification and transient compaction overhead. No universal ratio target is imposed. **Verification:** Benchmark/test: Full-directory accounting before/after.

**BACE-PERF-008 — Recovery.** Compression MUST not cause loss of committed data; recovery time impact shall be measured and bounded by deployment RTO policy. **Verification:** Benchmark/test: Full and incremental restore/replay.

**BACE-SEC-001 — Availability.** Malformed compressed frames MUST fail within bounded CPU/memory and never crash unrelated sessions or produce unbounded allocation. **Verification:** Benchmark/test: Decoder fuzzing and stress suites.

**BACE-SEC-002 — Confidentiality.** Compression dictionaries and deduplication MUST respect tenant and key boundaries; avoid cross-origin compressed secret/user input contexts susceptible to size inference. **Verification:** Benchmark/test: Cross-tenant and chosen-input threat tests.

**BACE-NFR-PORT-001 — Portable reproducibility.** Compressed payload decode and visible SQL results MUST be equivalent across supported architecture/OS builds, including different CPU SIMD paths. **Verification:** Benchmark/test: Golden corpus across architectures.

**BACE-NFR-OPS-001 — Observability.** Report codec selection, compression savings, decode/encode CPU, allocation, hot/cold coverage, recompression backlog and failures by object and workload. **Verification:** Benchmark/test: Operator API/Studio metrics integration.

## 8. Configuration, SQL and administrative contracts

Configuration is hierarchical: platform baseline → workload policy → database/schema/table → optional per-column override → explicit maintenance job. Priority conflicts must have defined resolution and persistent audit. Suggested policy keys include compression.profile, compression.max_cpu_percent, compression.max_decode_ns_per_kib, compression.min_saving_percent, compression.max_decoded_page_bytes, compression.background_io_mbps and compression.dictionary_scope. Exact names are proposals subject to Bradian catalog/API design approval.

Potential future SQL surface (non-implemented illustration): ALTER TABLE observations SET COMPRESSION = BALANCED; and OPTIMIZE TABLE observations RECOMPRESS. Actual parsing, privileges, catalog persistence, job progress, cancellation and rollback must be specified and tested before introducing these statements. Safe defaults must require no per-table tuning by ordinary application users.

**BACE-ADMIN-001 — Compression status and sizing.** Bradian Studio and CLI MUST show physical/logical bytes, compression ratio, codec breakdown, data/index/WAL/MVCC/projection footprints and correct accounting scope. **Verification:** Reconcile reported counters to actual files.

**BACE-ADMIN-002 — Controlled maintenance.** Recompression MUST be cancellable at safe checkpoints, permission checked, auditable, throttled and resumable or cleanly restartable. **Verification:** Cancel/restart and privilege revoke tests.

**BACE-ADMIN-003 — Explain integration.** EXPLAIN ANALYZE SHOULD attribute scanned compressed bytes, decoded bytes, decode time, codec and avoided materialization where helpful. **Verification:** Plan trace compared against runtime counters.

## 9. Implementation strategy and migration programme

**BACE-PLAN-001 — Gate A — Baseline and contracts.** Inventory all authoritative and derived stores, identify on-disk formats and codec IDs, establish golden files and benchmark fixtures. Freeze requirement traceability and interoperability boundaries. **Verification:** Document gate evidence, measured benchmark deltas, release risk and rollback process.

**BACE-PLAN-002 — Gate B — Common bounded codecs.** Harden registry, parameter validation, bounded decompression, RAW and LZ4/ZSTD wrappers; finish existing RLE/Delta/BitPack integrations with safe golden vectors. **Verification:** Document gate evidence, measured benchmark deltas, release risk and rollback process.

**BACE-PLAN-003 — Gate C — Adaptive analytical segments.** Introduce per-page selection, multiple pages/row groups, packed dictionaries, FOR and encoded-domain execution; maintain .derc compatibility. **Verification:** Document gate evidence, measured benchmark deltas, release risk and rollback process.

**BACE-PLAN-004 — Gate D — Relational and index paths.** Design new versioned BRPG-compatible page/value compression and independently assess B+tree prefix encoding under transactional load. **Verification:** Document gate evidence, measured benchmark deltas, release risk and rollback process.

**BACE-PLAN-005 — Gate E — WAL/MVCC/checkpoints.** Add bounded fast compression only where commit/replay tests satisfy durability and p99 latency budgets; optimize cold version chains and checkpoint work. **Verification:** Document gate evidence, measured benchmark deltas, release risk and rollback process.

**BACE-PLAN-006 — Gate F — Cluster/stream/spill.** Negotiate codecs for replication/federation and adapt to network conditions; optimize spill and streaming without changing sequence or snapshot semantics. **Verification:** Document gate evidence, measured benchmark deltas, release risk and rollback process.

**BACE-PLAN-007 — Gate G — Backup and lifecycle.** Implement streaming compressed backup and reference-safe incremental dedupe, plus generation-safe recompression and retention accounting. **Verification:** Document gate evidence, measured benchmark deltas, release risk and rollback process.

**BACE-PLAN-008 — Gate H — Studio/SQL and qualification.** Expose policies, status, explain data, progress and audit; execute compatibility, performance, failure and security release matrices. **Verification:** Document gate evidence, measured benchmark deltas, release risk and rollback process.

## 10. End-to-end verification and acceptance protocol

A release candidate must pass an evidence matrix covering (1) correctness and bit-exact data; (2) SQL isolation and HTAP snapshot parity; (3) durability and recovery under system/process crashes, disk full and torn writes; (4) codec corruption, missing dictionaries, unsupported versions and decompression bombs; (5) multi-tenant authorization and encryption; (6) performance under hot/cold, OLTP/OLAP/mixed and high-entropy data; (7) cross-platform and rolling-upgrade compatibility; (8) backup/restore, replication and federation recovery. A failed integrity or durability case blocks release regardless of storage savings.

Required benchmark corpus: random high-entropy bytes; repeated small domains; sorted and unsorted integers; sparse and dense nulls; long and short strings; Unicode, UUID and JSON; timestamp series with gaps; exact decimals; floating-point special values; highly skewed distributions; variable-size blobs; and realistic multi-table transactional/analytical fixtures. Each corpus is versioned, with generation seed and SHA-256 checksum stored alongside results.

### 10.1 Acceptance evidence per requirement

Each requirement shall be tracked as Not Started, Implementing, Tested, Blocked or Accepted, with code owner, ADR, test identifiers, measured metrics, baseline commit and release version. A feature shall never be marked completed merely because its API compiles; the corresponding reader/writer paths, durability tests, operational visibility and upgrade fixtures must pass.

### 10.2 Release blocker criteria

Release blockers include wrong SQL values; incorrect NULL/decimal/float semantics; any acknowledged transaction lost; reader failure on previously supported files without a documented mandatory migration; snapshot visibility violations; unbounded decompression; cross-tenant data exposure; corrupted restore; or unbounded OLTP latency due to maintenance. Performance gate violations require documented remediation and cannot be silently waived by switching test fixtures.

## 11. Risks, engineering decisions and ownership

**Risk — Compression vs latency.** Mitigate with per-workload limits, RAW fast path, cheap SIMD codecs and background high-ratio recompression.

**Risk — Transient compaction space.** Reserve headroom and admit jobs based on actual free space; publish generations atomically.

**Risk — Codec format proliferation.** Maintain one registry and compatibility corpus, while allowing store-specific envelopes.

**Risk — Unbounded decode memory.** Validate declared sizes, cap output during decompression and fuzz every decoder.

**Risk — History retention and dedupe.** Retain immutable references while live snapshots, replicas, backups or legal holds require them.

**Risk — Cross-tenant dictionary leakage.** Scope dictionaries/deduplication to authorization and cryptographic domains.

**Risk — Mixed-version distributed upgrades.** Negotiate codec capabilities and prevent unsupported writes to older replicas.

**Risk — Benchmark gaming.** Publish full physical footprint, workloads, caching, resource costs and raw baseline methodology.

## 12. Master-SRS traceability and source notes

The following crosswalk is derived from the uploaded BDS-MASTER-001 revision 1.1: §2 integrated platform; §4 relational pages/WAL/MVCC; §5 .derc and vectorized OLAP; §6 one authoritative commit domain; §7 clusters/federation; §10 encrypted-at-rest, parsers and access control; §11 backup/PITR/retention; §12 data interchange; §13 resource governance and telemetry; §14 measurable engineering gates. BACE details beyond the master document, including named codec candidates, exact selection formula, implementation gates and proposed regression thresholds, are new normative proposals, not statements that the master SRS already specified them.

Repository baseline examined: crates/dap-storage/src/codec/mod.rs, crates/dap-storage/src/segment/{format,writer}.rs, docs/adr/0002-derc-segment-format.md and docs/implementation/BRADIAN_SOURCE_INVENTORY.md. The observed .derc writer uses plain/plain UTF-8/dictionary encodings and optional default ZSTD level 3, while RLE/Delta/BitPacked remain reserved for persistent writing. The row store is architecturally separate; BACE must not mistake analytical compression for full-system coverage.

Source records: [1] Vitalworks Limited, Bradian Database Systems Master Product Architecture & SRS, BDS-MASTER-001, Rev. 1.1, 12 Nov. 2026 (user-supplied). [2] Bradian source repository, https://github.com/socaya/bradian-db, reviewed 13 Nov. 2026. [3] Bradian ADR 0002, .derc native segment format v1, in repository docs/adr/. Standards, patents and licensing of proposed specialist codecs require separate due diligence before product adoption.

## Appendix A. Requirement identifier conventions

BACE-FR-[SUBSYSTEM]-NNN denotes functional requirements; BACE-PERF/SEC/NFR denotes measurable quality requirements; BACE-FMT governs persistent and wire compatibility; BACE-CODEC governs codec families; BACE-PLAN captures engineering release gates. Status is proposed unless explicitly supported by evidence in the source inventory. Revisions shall preserve identifiers or publish a change log and cross-reference removed/renamed IDs.

## Appendix B. Definition of done

BACE is production-ready for a given deployment profile only when every advertised subsystem has a documented policy, implemented bounded decoder, compatibility contract, integrity and authorization controls, measured performance within approved budgets, fault-safe recovery, operational metrics, migration and rollback guidance, and independently reproducible test evidence. Partial functionality must be labeled by subsystem and profile rather than presented as universal product-wide compression.