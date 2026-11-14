# Bradian comprehensive gaps analysis

14 November 2026. Baseline commit `5a0cb2d`. 852 workspace tests, 0 failures.

Normative sources cross-checked for this analysis:

- [Bradian Master Architecture SRS v1.1](../Bradian_Master_Architecture_SRS_v1.1.md) — 147 sections. Authoritative.
- [BACE Comprehensive Requirements SRS v1.0](../BACE_Comprehensive_Requirements_SRS_v1.0.md) — 32 functional requirements across 8 subsystems. **Proposed engineering baseline — not implemented.**
- [Master traceability matrix](BRADIAN_MASTER_TRACEABILITY.md) — 60 requirement IDs, derivative of §16 + §31.
- [Module review](BRADIAN_MODULE_REVIEW.md) — 35 modules audit.
- [Implementation plan](BRADIAN_COMPREHENSIVE_IMPLEMENTATION_PLAN.md) — gate-by-gate decomposition.
- [Gap register](BRADIAN_IMPLEMENTATION_GAPS.md) — the running checklist.

## 1. Executive summary

Bradian ships a verifiably correct, honestly scoped single-process
Gate 1 kernel. The gap between what the SRS **promises** and what is
**shipped** is large. This document separates that gap into five
discrete categories so readers can see at a glance whether a given
item is a **defect**, a **deliberate deviation**, a **feature
shortfall**, a **design blocker** or an **infrastructure blocker**.

| Category | Count | Severity |
|---|---|---|
| **Invariant obligations (§20 MUST clauses)** — cross-cutting, release-blocking | 5 | Critical |
| **Protocol MUST clauses (§21 global txn, §22 storage, §23 security)** | 20+ | Critical |
| **Feature shortfalls (§16 + §31 PARTIAL/OPEN)** | 60 | Mixed |
| **BACE functional requirements (BACE §4 FR + §5 codecs + §6 format)** | 32 FR + 11 codecs + 5 format MUSTs | Critical (cross-cutting) |
| **Scoped deviations documented in ADRs** | 9 | Not defects |
| **§13 owner-decision blockers** | 12 | Unblockable without owner |
| **Infrastructure blockers (hardware, cross-platform CI)** | 7 | Unblockable without infra |

Honest overall assessment: **the kernel shipped is correct for its
single-process Gate 1 scope.** Everything above Gate 1 — and most of
the invariant obligations in §20 — requires either an owner decision
or an infrastructure that this plan does not procure. No further
code-only work in `bradian-relational` meaningfully closes these gaps.

## 2. Critical finding — normative weight is uneven across the SRS

The traceability matrix treats every requirement ID as equivalent,
marked PARTIAL / OPEN / INHERITED. The SRS itself does **not** treat
them equivalently. There are three distinct normative tiers:

1. **§20 architecture invariants** — the five numbered invariants
   (INV-001..INV-005) use explicit **MUST** and **MUST NOT**. A
   violation here is a release blocker and takes precedence over any
   performance target (§20.2: "No lower-level change may violate an
   invariant to achieve a performance target").
2. **§21–§23 protocol specifications** — distributed commit, storage
   recovery, security enforcement all contain explicit MUST clauses.
3. **§16 / §31 formal registers** — descriptive feature lists in
   imperative noun-phrase form. No MUST language. These describe
   capabilities, not contracts.

**Consequence:** a `PARTIAL` row in the matrix can mean two very
different things: (a) a descriptive feature is incomplete (acceptable
with disclosure per SRS §33.2), or (b) a MUST obligation is unmet
(release blocker). The matrix does not distinguish.

**This gaps document separates them.** §3 below lists the invariant
and protocol MUST obligations first; §4 and later list feature
shortfalls separately.

## 3. Invariant and protocol MUST obligations

These are the **release-blocking contract gaps**. Each has normative
MUST/MUST NOT language in the SRS. Each has an explicit §13
owner-decision prerequisite or infrastructure prerequisite.

### 3.1 §20 architecture invariants

| Invariant | SRS verbatim MUST / MUST NOT | Current compliance | Blocker |
|---|---|---|---|
| **INV-001** Commit correctness | "No transaction is acknowledged as durable until the chosen local and, where requested, quorum durability obligations are satisfied." | **Local: compliant.** Quorum: **not applicable** — no distributed commit yet. | Blocked on Gate 4+ consensus choice. |
| **INV-002** One authoritative state | "A query combining data structures **MUST** bind all inputs to a consistent snapshot or explicitly declare bounded staleness." | **Row paths: compliant** via snapshot isolation. **HTAP cross-store: n/a** — no column projection yet. | Blocked on Gate 3 HTAP. |
| **INV-003** No authorization gap | "Every access path … **MUST** evaluate an authenticated and versioned authorization context, or demonstrably inherit a narrower, validated decision." | **PARTIAL.** Local CLI enforces private-root UID/mode (ADR 0053/0057). No end-to-end authorization context propagation across SQL / RPC / caches / jobs (this is BRD-SEC-008, OPEN). | Blocked on enterprise identity + TLS/mTLS owner decisions. |
| **INV-004** No unsafe automatic availability | "A strongly consistent shard that loses quorum **MUST** reject writes requiring the quorum." | **N/A — no quorum yet.** | Blocked on Gate 4 consensus. |
| **INV-005** Recoverable lifecycle | "Every persistent format, catalog migration, schema job, shard relocation and encryption-key transition **MUST** have an enumerated state machine, durable progress markers, restart behavior, observability and either a safe rollback or a documented forward-recovery path." | **PARTIAL.** G17/G23 migrations have state machines + rollback. Schema jobs: exclusive ALTER only (G36). Shard relocation: n/a. Encryption-key transition: not implemented. | Partial — remaining items blocked by Gates 2/4/8. |

### 3.2 §20.2 signed-ADR obligations

The SRS **requires signed ADRs** on seven specific topics before
Gate advancement. Current state:

| §20.2 topic | ADR obligation | Status |
|---|---|---|
| Consensus | Must have signed ADR before Gate 4 | **No ADR.** §13 blocker. |
| Global timestamps | Must have signed ADR before Gate 5 | **No ADR.** §13 blocker. |
| Locking / MVCC | ADRs exist (0025, 0027) | **Compliant for single-process scope.** |
| Distributed atomic commit | Must have signed ADR before Gate 5 | **No ADR.** §13 blocker. |
| WAL recovery | ADRs exist (0003, 0008, 0014) | **Compliant.** |
| Collations | Must have signed ADR before Gate 2 SQL conformance | **No ADR.** Current catalog is ASCII-only. |
| Key custody | Must have signed ADR before Gate 8 | **No ADR.** §13 blocker. |

### 3.3 §20.2 fabricated-feature prohibition

"Operators **MUST NOT** be presented with a fabricated complete
feature list." — §20.2.

**Compliant.** The CLAUDE.md "no scaffolding-as-feature" rule and the
per-slice verification reports enforce this. Every PARTIAL/OPEN in
the matrix is acknowledged, not hidden.

### 3.4 Cross-cutting §21–§23 protocol MUSTs

Representative examples. Full list in SRS §21–§23.

| SRS clause | Verbatim | Status |
|---|---|---|
| §21 distributed commit | Multiple MUST clauses on cross-shard decision, coordinator recovery, safe-time | **Not applicable — no distributed commit.** BRD-DTX-001/002/003 OPEN. |
| §22 native storage | "WAL durability MUST survive torn writes and partial writes within a frame." | **Compliant for CRC + bounded-tail trim.** Power-loss on physical device unqualified (infrastructure blocker). |
| §22 native storage | "Corruption MUST be detected and refused before any recovery decision." | **Compliant.** Every page has a CRC; every WAL frame is CRC-checked; fault-recovery slices prove refusal. |
| §23 security enforcement | TLS/mTLS MUST on remote transports; authenticated encryption MUST for off-host payloads | **Not applicable — no remote transports.** BRD-SEC-003 PARTIAL. |
| §23 security enforcement | Audit MUST be tamper-evident; audit MUST survive archive/restore | **Not implemented.** No native audit log. BRD-SEC-005 PARTIAL. |
| §33.1 security scenario | "A denied access **MUST** remain denied under every plan" | **N/A — no plan optimizer with authorization context.** |
| §33.2 release-note | "User-facing release notes **MUST** identify implemented SQL grammar, supported storage versions, deployment modes, secure defaults and known limitations." | **Compliant.** `BRADIAN_SQL_SUBSET.md`, `BRADIAN_ISOLATION_LEVELS.md`, operations guides, verification reports all do this. |

## 4. Feature-shortfall gaps (§16 + §31 matrix)

These are the 60 requirement IDs from the formal registers. Every
one either traces to a shipped module (PARTIAL) or is explicitly
OPEN. **No requirement ID is silently unaddressed.** The Explore
audit confirmed the matrix contains exactly the SRS set.

Status distribution:

- **Fully PARTIAL with ongoing shipped evidence:** 28 (relational, txn, storage, DDL, migration, backup, portability, ops, SQL profile).
- **INHERITED (DER analytical, not Bradian-integrated):** 4 (BRD-STO-002, BRD-OLAP-001/002/003).
- **OPEN (no backing module):** 28 (HTAP, cluster, distributed-txn, federation, job, OPS-007, most SEC-*).

Detailed per-ID status is in `BRADIAN_MASTER_TRACEABILITY.md`. The
gap vs the SRS prose per module is in `BRADIAN_MODULE_REVIEW.md` and
the earlier conformance review. I will not re-tabulate here; instead
I highlight the **structural** gaps.

### 4.1 Relational and SQL (§16: BRD-REL-*, BRD-SQL-*)

- **BRD-REL-002 PK/FK/UNIQUE/CHECK** — PK enforced; secondary and FK
  refused at `CREATE TABLE` (not silently accepted). Scoped
  deviation per ADR 0009 + `BRADIAN_SQL_SUBSET.md`. CHECK is not
  parsed. Referential actions (CASCADE/SET NULL/RESTRICT) not
  parsed.
- **BRD-REL-003 prepared statements** — Not implemented. Tracked in
  plan §4.3.
- **BRD-REL-003 documented SQL subset** — **Compliant.**
  `BRADIAN_SQL_SUBSET.md` with 36 claims proven by `sql_subset_corpus.rs`.
- **BRD-REL-004 TimestampTz / Uuid** — Variants exist but
  unreachable through SQL. Scoped deviation (Slice I test).
- **BRD-SQL-005 versioned SQL profile, stable error classes** —
  Stable `SqlError` enum. Not versioned.
- **BRD-SQL-006 collation / temporal / decimal / NULL** — Decimal
  and NULL compliant. Collation open (needs §20.2 ADR). Temporal SQL
  not reachable.

### 4.2 Transactions (§16: BRD-TXN-*)

- **BRD-TXN-001 ACID, savepoints** — Single-process compliant.
  Local atomicity, rollback, savepoints, release.
- **BRD-TXN-002 MVCC snapshots and implemented isolation levels** —
  Snapshot + opt-in bounded read validation. **Not SQL-standard
  SERIALIZABLE.** Documented in `BRADIAN_ISOLATION_LEVELS.md` §7.
- **BRD-TXN-003 deadlock handling, lock timeouts, revision GC** —
  Revision GC via manual retention advancement. **Deadlock handling
  and lock timeouts: not implemented.** Single-process means lock
  contention is bounded, but the SRS wording is stronger.

### 4.3 Storage (§16: BRD-STO-*, §31: BRD-STO-004..006)

- **BRD-STO-001 native row pages, primary/secondary indexes,
  checksums** — Primary (PK) B+tree compliant. **Secondary index
  implementation is not present.** Scoped per plan §4.1.
- **BRD-STO-002 native columnar segments, atomic manifests** —
  INHERITED from `.derc`. Not integrated with transactional state.
  BACE §4.2 depends on this.
- **BRD-STO-003 WAL, checkpoints, restore, PITR, snapshot
  consistency** — WAL/checkpoint/restore compliant. **PITR not
  implemented.** Plan §8.3.
- **BRD-STO-004 WAL / page-LSN / checkpoint / torn-page protocol** —
  Compliant for software-detectable cases (fault_recovery slices).
  Physical-device torn writes infrastructure-blocked.
- **BRD-STO-005 crash-safe B+tree split, allocator and page reuse**
  — **Scoped deviation.** Shipped is immutable per-generation
  B+tree + WAL-backed delta overlay (ADR 0007, 0010, 0012). SRS
  wording implies in-place mutable. Plan §5.1.
- **BRD-STO-006 verified format migration and corruption refusal** —
  Compliant for legacy_migration + history_migration + CRC refusals.
  Reader fuzzing pending.

### 4.4 OLAP and HTAP (§16: BRD-OLAP-*, BRD-HTAP-*)

- **BRD-OLAP-001/002/003** — INHERITED from DER. Vectorized
  operators, CBO, incremental views exist in `dap-runtime` /
  `dap-views` but are not fed by committed transactional state.
- **BRD-HTAP-001/002/003** — OPEN. Requires commit-consistent
  column projection + watermark + staleness contract. Blocked on
  mutable storage (plan §5.1).

### 4.5 Cluster, distributed txn, federation (§16+§31: BRD-CL-*,
BRD-DTX-*, BRD-FED-*)

**All OPEN.** Gates 4–7. Blocked on §13 owner decisions:
consensus choice, global timestamp source, PKI/identity model,
federation identity contract. No backing modules exist. The SRS is
explicit (§32 Gate 5): "Cross-shard consistency is a standalone
gate and cannot be declared complete because local ACID passes."

### 4.6 Security (§16: BRD-SEC-001..007, §31: BRD-SEC-008..012)

- **BRD-SEC-001 authentication, MFA, secure sessions** — Bootstrap
  token + private-root UID/mode. Not enterprise identity.
- **BRD-SEC-002 RBAC/ABAC, row/column/tenant policies end-to-end**
  — Policy helpers in `dap-security`, no authoritative enforcement.
- **BRD-SEC-003 TLS/mTLS and managed at-rest encryption** —
  Primitives exist; no native transport.
- **BRD-SEC-004–012** — All PARTIAL or OPEN. Secrets rotation,
  audit tamper-evidence, context propagation (BRD-SEC-008),
  diagnostics disclosure controls, administrator separation,
  revocation propagation, release-blocking controls — none are
  end-to-end enforced.

### 4.7 Studio, API, drivers, jobs (§16: BRD-STU-*, BRD-API-*,
§31: BRD-JOB-001)

- **BRD-STU-001/002** — React shell only. §13 blocker on auth
  posture.
- **BRD-API-001** — Loopback bootstrap only. §13 blocker on driver
  wire protocol.
- **BRD-JOB-001** — OPEN. No durable job system.

### 4.8 Operations, portability, migration, DR (§31: BRD-OPS-*,
BRD-PORT-*, BRD-MIG-*, BRD-DR-*)

- **BRD-OPS-001 observability, limits, cancellation, upgrade** —
  12 admission budgets + `ScanControl` cancellation.
  **Upgrade support and broader observability (Studio metrics)
  open.**
- **BRD-OPS-007 global resource reservations and fair scheduling**
  — OPEN. Depends on §13 global memory accounting decision.
- **BRD-OPS-008 query regression and workload replay** —
  Inherited benchmark harness. External reproducibility open.
- **BRD-PORT-001 Windows/Linux/macOS portability** — Linux only in
  practice. Infrastructure blocker.
- **BRD-MIG-001/002 compatibility + CDC catch-up** — Offline cutover
  proven. **CDC not implemented** — scoped deviation.
- **BRD-DR-001/002 cluster-consistent backup, PITR, immutable
  archives, regional promotion** — Local backup compliant.
  **PITR, cluster-consistent, immutable archives, regional
  promotion all open.**

## 5. BACE integration gaps

**All 32 BACE functional requirements are unimplemented.** The only
compression in-tree is a hardcoded ZSTD-3 wrap on `.derc` columnar
pages (BACE §6). Nothing in `bradian-relational` implements the
BACE codec registry, policy service, selector, envelope or
admission. BACE cross-cuts 10+ shipped modules.

### 5.1 BACE architecture-contract MUSTs (§3)

| BACE ID | Verbatim MUST | Status |
|---|---|---|
| **BACE-ARCH-001** Central policy service | A versioned policy model **MUST** resolve global, workload, database, table, column, tier and job-level options. | **No policy service.** |
| **BACE-ARCH-002** Codec registry | Each codec **MUST** publish stable identifier, format version, supported types, parameters, decoder bounds, determinism and SIMD availability. Unknown critical codecs **MUST** fail closed. | **No registry.** |
| **BACE-ARCH-003** No dual authority | BACE **MUST** treat relational committed state and commit log as authoritative; column projections remain derivatives. | **N/A — no projections yet.** |
| **BACE-ARCH-004** Isolation of compression work | Heavy candidate evaluation and recompression **MUST** use dedicated resource-governed jobs. | **No job system (BRD-JOB-001 OPEN).** |
| **BACE-ARCH-005** Versioned compatibility | On-disk/wire compression metadata **MUST** be self-describing enough for bounded decoding and host-independent. | **No envelope.** |
| **BACE-ARCH-006** Encryption ordering | Compression **MUST** precede authenticated encryption on the chosen payload. | **No encryption pipeline (BRD-SEC-003 PARTIAL).** |

### 5.2 BACE functional requirements by subsystem (§4)

All 32 FRs OPEN. Summary by section:

| Section | FRs | Current state |
|---|---|---|
| §4.1 OLTP row store + page cache | BACE-FR-ROW-001..004 (4) | None; BRPG envelope cannot carry codec ID today. |
| §4.2 Columnar OLAP + .derc | BACE-FR-COL-001..005 (5) | Hardcoded ZSTD-3 only; no codec selection, no page statistics API, no encoding-aware filter pushdown. |
| §4.3 HTAP projections | BACE-FR-HTAP-001..003 (3) | No HTAP projection exists. |
| §4.4 Indexes, search, metadata | BACE-FR-IDX-001..003 (3) | B+tree keys plain; catalog strings uncompressed. |
| §4.5 WAL, checkpoints, MVCC | BACE-FR-DUR-001..004 (4) | WAL frames uncompressed; MVCC chains plain. |
| §4.6 Distributed replication + federation | BACE-FR-NET-001..004 (4) | N/A — no distributed yet. |
| §4.7 Streaming / ingestion / spill | BACE-FR-RUN-001..004 (4) | No compressed spill; Arrow/Parquet adaptors not present. |
| §4.8 Backups / recovery / lifecycle | BACE-FR-BKP-001..004 (4) | Backup payloads uncompressed; no incremental PITR dedup. |

### 5.3 BACE codec catalogue (§5)

Eleven codec IDs defined. **None implemented** in the BACE sense (no
codec registry, no capability negotiation, no selection). ZSTD-3 on
`.derc` is a hardcoded invocation of the external `zstd` crate, not
a registered BACE codec.

### 5.4 BACE persistent-format MUSTs (§6)

| BACE ID | Verbatim MUST | Status |
|---|---|---|
| **BACE-FMT-001** Strict envelope | Every compressed durable object **MUST** carry validated version, codec ID, flags, encoded length, decoded length, bounds and integrity material. | **No envelope.** |
| **BACE-FMT-002** Atomic publication | Compression write paths **MUST** use durable staging and atomic manifest/generation publication. | **N/A — no compressed writes.** |
| **BACE-FMT-003** Bounded decompression | Readers **MUST** reject declared decoded sizes above configured page/frame maximum before allocating. | **N/A — no decompressor.** |
| **BACE-FMT-004** Rollback-safe upgrades | Upgrades **MUST** preserve legacy readers or require formally verified backed-up migration with explicit downgrade restriction. | **N/A — no upgrade yet.** |
| **BACE-FMT-005** Checksums vs authenticity | Encrypted or untrusted payloads **MUST** use authenticated integrity. CRC detects accidental corruption only. | **CRC only today. Authenticated integrity for off-host open.** |

### 5.5 BACE non-functional MUSTs (§7)

All eight performance gates (BACE-PERF-001..008) are explicitly
**proposed, not empirical**. No measurement harness. BACE-NFR-OPS-001
(observability) requires Studio metrics integration (OPEN).

### 5.6 BACE delivery dependencies

Before any BACE implementation code can land, the following must
exist. None currently do:

1. Signed ADR for BACE central policy service (§20.2 class).
2. Signed ADR for BACE codec registry and capability negotiation.
3. Signed ADR for BRPG v2 feature-flag page format (§4.1).
4. Signed ADR for compressed envelope versioning (§6).
5. §13 owner sign-off on whether BACE starts with backup
   (cold path, lowest risk) or WAL (highest leverage, highest risk).

## 6. Scoped deviations (deliberate, documented)

Nine places where `bradian-relational` *does not* literally match
the SRS prose. Each is a documented design choice, not a defect.

| Deviation | SRS prose that implies otherwise | ADR scoping it |
|---|---|---|
| Immutable B+tree generations | BRD-STO-005 | ADR 0007, 0010, 0012 |
| No FK / UNIQUE / CHECK enforcement | BRD-REL-002 | ADR 0009 + `BRADIAN_SQL_SUBSET.md` |
| Snapshot + bounded read-validation (not SERIALIZABLE) | BRD-TXN-002 | ADR 0037 + `BRADIAN_ISOLATION_LEVELS.md` §7 |
| Exclusive ALTER (no concurrent online DDL) | BRD-DDL-001 | ADR 0031, 0036 + plan §4.2 |
| TimestampTz / Uuid unreachable from SQL | BRD-REL-004 | `BRADIAN_SQL_SUBSET.md` §1 + Slice I test |
| Offline-only migration (no CDC) | BRD-MIG-002 | ADR 0017, 0022, 0025 |
| Local-process only (no distributed/multi-process) | BRD-TXN-*, BRD-DR-*, every BRD-CL-* | Entire crate scope; plan §6–§10 |
| POSIX-only `local_access` | BRD-SEC-001, BRD-PORT-001 | ADR 0053 + plan §3.3 |
| Append-only catalog journal (no compactor) | BRD-REL-001 | ADR 0016, 0054 + plan §6.1 |

## 7. Owner-decision blockers (§13)

**Twelve** decisions must be owner-signed before dependent work
can honestly begin. Code-only slicing can no longer move these
forward.

| Decision | Blocks | Severity |
|---|---|---|
| Global process-memory accounting model | G1-MEMORY global budget, BRD-OPS-007 | High |
| Fault-injection design (trait vs filesystem shim) | G1-FAULTS, BRD-STO-004 real-ENOSPC | High |
| External SQL reference corpus choice | G1-REFERENCE acceptance, BRD-SQL-005/006 | Medium |
| Parent-path trust policy | G1-LOCAL-ACCESS parent trust, INV-003 | Medium |
| ACL inspection platform binding | G1-LOCAL-ACCESS ACL | Low |
| Online-DDL protocol (ADR 0062–0064) | Gate 2 schema evolution, BRD-DDL-001 | High |
| Mutable storage protocol (ADR 0065–0069) | Gate 3 HTAP, BRD-STO-005 | **Critical** |
| PKI vs self-generated identities | Gate 4 TLS/mTLS, INV-003, BRD-SEC-001/003 | **Critical** |
| Consensus choice (Raft or alternatives) | Gate 4 replication, INV-004, BRD-CL-* | **Critical** |
| Global-timestamp source (HLC vs TrueTime) | Gate 5 distributed commit, BRD-DTX-*, INV-002 | **Critical** |
| Studio auth posture (local-only vs enterprise) | Gate 6 Studio, BRD-STU-001/002 | Medium |
| Driver wire protocol (PG-compat vs native) | Driver workstream, BRD-API-001 | High |

**BACE-related owner decisions additional to the above:**

| Decision | Blocks |
|---|---|
| BACE central-policy service shape | BACE-ARCH-001 and every §4 FR |
| Codec registry capability-negotiation wire format | BACE-ARCH-002/005, BACE-FMT-001 |
| BRPG v2 feature-flag page layout | BACE-FR-ROW-002, BACE-FMT-001 |
| First BACE adapter target (backup vs WAL vs .derc) | Which subsystem lands first |

## 8. Infrastructure blockers

Items that need resources outside the code review scope.

| Blocker | Blocks |
|---|---|
| Physical hardware lab | BRD-STO-004 power-loss, BACE-PERF-* |
| Device-level torn-write test rig | BRD-STO-004/006 |
| Cross-platform (Windows/macOS) CI | BRD-PORT-001, G1-PORTABILITY |
| External SQL reference suite host | G1-REFERENCE, BRD-SQL-005 |
| Multi-node network lab | BRD-CL-*, BRD-DTX-*, BACE-FR-NET-* |
| Independent penetration-review provider | BRD-SEC-012 release-blocking review |
| Published benchmark hardware | BRD-OPS-008, BACE-PERF-* |

## 9. Still-achievable code-only follow-ups

These remain inside Gate 1 scope and do not require §13 or
infrastructure. Each is a single vertical slice.

| Item | Scope |
|---|---|
| `DomainSqlEngine::open_with_limits` wrapper | Follow-up to Slice L; thread `MultiTableStore::open_with_limits` + `CatalogLimits` through the SQL frontend. |
| Observational `resource_usage()` aggregator | Report combined counts across catalog + domain + MVCC. No admission; operator diagnostics only. |
| Expand fault-recovery matrix | Lock-file deletion mid-session, WAL header-only-no-body, parallel-open race under the existing lock. |
| Range `ORDER BY` positive corpus | Only possible if we accept range predicates; today they're correctly rejected. So this is **not** achievable — flagged honestly in `BRADIAN_SQL_SUBSET.md` §14. |
| Cross-engine isolation proofs | Additional citations in `BRADIAN_ISOLATION_LEVELS.md` from `durable_mvcc` and `history_sql` schedules. |

Everything else is either §13-blocked, infra-blocked, or a BACE
workstream that needs the §20.2 signed-ADR review.

## 10. Priority matrix

Severity bands for all gaps above, highest-priority first.

### Priority 0 — invariant / release-blocking

1. **INV-003 authorization context propagation (BRD-SEC-008)** — explicit MUST across every access path. Requires enterprise identity + TLS/mTLS.
2. **INV-005 recoverable lifecycle for key custody and schema jobs** — MUST have state machine + rollback. Partial.
3. **BRD-STO-005 crash-safe B+tree with allocator + page reuse** — mutable storage is Gate 3 precondition.
4. **BRD-TXN-003 deadlock handling + lock timeouts** — MUST is implicit in §22; shipped is single-process bounded.
5. **BACE-FMT-001 strict compressed envelope** — MUST before any compressed durable write.

### Priority 1 — §13 owner-decision unblocks

6. BACE central policy service + codec registry (ADRs 0059/0060).
7. Mutable storage ADRs (0065–0069).
8. Online-DDL protocol ADRs (0062–0064).
9. PKI / identity / consensus ADRs (0074–0078).

### Priority 2 — feature shortfalls with no ADR obstacle

10. Prepared statements (BRD-REL-003).
11. JSON type (BRD-REL-004).
12. Secondary index implementation (BRD-STO-001).
13. PITR (BRD-STO-003 / BRD-DR-001).
14. Collation ADR (§20.2) and basic collation support (BRD-SQL-006).
15. Native audit log (BRD-SEC-005).

### Priority 3 — infrastructure / qualification

16. Cross-platform installer + lock lifecycle (BRD-PORT-001).
17. Device-level fault lab (BRD-STO-004).
18. Independent penetration review (BRD-SEC-012).
19. Published benchmark corpus (BRD-OPS-008, BACE-PERF-*).

### Priority 4 — code-only hygiene

20. `DomainSqlEngine::open_with_limits` wrapper.
21. `resource_usage()` aggregator.
22. Fault-recovery matrix extensions.

## 11. Finding summary

1. **The gap matrix is complete.** Every SRS v1.1 requirement ID
   from §16 and §31 appears in the traceability matrix. No ID is
   silently unaddressed.
2. **The gap matrix is uneven in normative weight.** Some PARTIAL
   rows are feature shortfalls (release-acceptable under §33.2);
   others are invariant MUST obligations (release-blocking under
   §20). This document separates them; the matrix should
   eventually adopt the same distinction.
3. **BACE is a parallel SRS with zero backing.** 32 FRs + 11
   codecs + 5 format MUSTs + 8 performance gates — none
   implemented. BACE must land before any §20 invariant that
   requires compressed storage can be met.
4. **The signed-ADR obligation in §20.2 is partly unmet.**
   WAL, locking/MVCC have ADRs. Consensus, global timestamps,
   distributed atomic commit, collations and key custody do
   **not** have signed ADRs yet. These are prerequisites to
   their respective gates.
5. **Code-only slicing is exhausted.** No more than 2–3 marginal
   slices remain inside Gate 1 scope before a §13 decision or an
   infrastructure procurement is required.

---

Pointers:

- [Master SRS v1.1](../Bradian_Master_Architecture_SRS_v1.1.md)
- [BACE SRS v1.0](../BACE_Comprehensive_Requirements_SRS_v1.0.md)
- [Master traceability](BRADIAN_MASTER_TRACEABILITY.md)
- [Module review](BRADIAN_MODULE_REVIEW.md)
- [Implementation plan](BRADIAN_COMPREHENSIVE_IMPLEMENTATION_PLAN.md)
- [Gap register (short form)](BRADIAN_IMPLEMENTATION_GAPS.md)
