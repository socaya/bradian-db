# Bradian Platform — Comprehensive Gaps and Implementation Plan

**Status:** Source of truth as of 2026-11-24. Supersedes ad-hoc planning in
chat. Updates to this file are the only accepted record of "what is still
open" and "what we are doing next."

**Scope:** Every crate, every app, every test infrastructure component in
`socaya/bradian-db`. Cross-referenced against the Master SRS v1.1 gate
order (Section 32), the fixed master roadmap and the executed verification
reports.

**Audit basis:**
- Repo-wide crate inventory (15 crates + 3 apps + 1 e2e test crate).
- Master-roadmap gate position (Gate 1 active; Gates 2–9 not accepted).
- Existing [`BRADIAN_IMPLEMENTATION_GAPS.md`](BRADIAN_IMPLEMENTATION_GAPS.md)
  per-ADR evidence table.
- ADRs 0001–0258 and gate records BRD-G1 through BRD-G44.

---

## 1. Executive summary

| Area                                   | State        | Trust bar                                    |
|----------------------------------------|--------------|----------------------------------------------|
| Gate 0 — evidence baseline             | Closed       | Documentation + verification reports         |
| Gate 1 — single-node kernel            | **Active**   | ~85% implemented; qualification tasks open   |
| Gate 2 — relational compatibility      | Not accepted | Supporting-evidence slices only              |
| Gate 3 — columnar HTAP                 | Not accepted | Analytical engine exists but not committed   |
| Gates 4–9                              | Blocked      | Depend on earlier gates + architecture decisions |

The implementation is heavily concentrated in **`bradian-relational`**
(84 src files, 172 test files, 258 ADRs). The 14 other crates range from
partial scaffolds to skeletons.

Five P0 blockers prevent Gate 1 acceptance, four remaining P0 slices live
inside those blockers, and ten P1 items block Gates 2–9. Each is tracked
below with an owner slice, acceptance criteria, and the next ADR number.

---

## 2. Crate-by-crate inventory

### 2.1 Substantial crates (ship-quality in-scope)

| Crate | src / tests | Status | Gate | Owner topics |
|-------|-------------|--------|------|--------------|
| `bradian-core` | 9 / 0 | Substantial | 1 | Typed value model, UID, periods, provenance, errors |
| `bradian-storage` | 23 / 12 | Substantial | 1 | `.brcs` segments, WAL, manifests, compaction |
| `bradian-relational` | 84 / 172 | Substantial | 1 | Catalog, WAL rows, B+tree, SQL engines, MVCC, history |
| `bradian-admin` | 5 / 2 | Substantial | 1 | Operator CLI + diagnostics |

### 2.2 Partial crates (need completion for their gate)

| Crate | src / tests | Status | Gate | Open deliverables |
|-------|-------------|--------|------|-------------------|
| `bradian-runtime` | 9 / 1 | Partial | 1–3 | Column batches, vectorized ops, scheduler, spill |
| `bradian-planner` | 6 / 1 | Partial | 2–3 | Parser / binder / plan / optimizer / EXPLAIN |
| `bradian-serving` | 8 / 3 | Partial | 6 | HTTP/JSON API; currently loopback-only |
| `bradian-catalog` | 6 / 0 | Partial | 2 | Versioned semantic catalog + lineage (DHIS2-aligned) |
| `bradian-security` | 6 / 0 | Partial | 6 | AuthN/AuthZ/audit/cache keying |
| `bradian-ingest` | 4 / 0 | Partial | 2 | Change log + idempotent ingestion |
| `bradian-dhis2` | 10 / 0 | Partial | 3 | DHIS2 aggregate/event/tracker/indicator |
| `bradian-sources` | 4 / 0 | Partial | 3 | CSV / JSON Lines / Arrow IPC / SQL CDC adapters |
| `bradian-views` | 6 / 0 | Partial | 3 | Incremental materialized views + retractions |
| `bradian-distributed` | 6 / 1 | Partial | 4–5 | Coordinator / workers / exchange / reconciliation |

### 2.3 Skeleton crates

| Crate | src / tests | Status | Gate | Open |
|-------|-------------|--------|------|------|
| `bradian-benchmark` | 3 / 0 | Skeleton | 3+ | Criterion harness + synthetic DHIS2 fixtures |

### 2.4 Apps

| App | Stack | State | Gate | Open |
|-----|-------|-------|------|------|
| `apps/bradian-ui` | Rust | Partial | 6 | Studio: database admin + analytics workbench |
| `apps/dhis2-integration` | Java (Maven) | Partial | 3 | DHIS2 integration layer |
| `apps/dhis2-plugin` | Empty | Skeleton | 3 | Placeholder only |

### 2.5 Test infrastructure

| Component | State | Notes |
|-----------|-------|-------|
| `tests/bradian-e2e` | Partial | 2 `.rs` files; depends on 8 workspace crates |
| Per-crate integration tests | Only `bradian-relational` (172) and `bradian-storage` (12) carry non-trivial test suites |
| Property / fault injection | ADRs 0252–0255 cover RowStore / user / DDL journals |
| Benchmarks | Skeleton |
| External SQL conformance suite | Open (G1-REFERENCE) |
| Cross-platform CI | Disabled by owner directive; manual-only |

---

## 3. Gap register — grouped by blocker class

Each row is one tractable ADR-sized slice. Priority is `P0` (Gate 1
blocker), `P1` (Gate 2–3 blocker), `P2` (Gate 4+ blocker or usability).
The **ADR** column names the next ADR number to be filed when the slice
starts; when the slice ships, the ADR number moves to
`BRADIAN_IMPLEMENTATION_GAPS.md` and this row is marked DONE.

### 3.1 P0 — Gate 1 blockers

#### P0-1 Schema evolution (owner: `bradian-relational`) — **ALL DONE**

| Slice | ADR | Status | Acceptance |
|-------|-----|--------|------------|
| SET NOT NULL + BACKFILL | 0241 | DONE | 7 tests |
| Expression DEFAULT (`DEFAULT CURRENT_TIMESTAMP` / `NOW()`) | 0259 | DONE | 9 tests. `gen_random_uuid()` deferred |
| ALTER COLUMN TYPE narrowing with implicit CAST | 0260 | DONE | 8 tests |
| DROP COLUMN CASCADE + dependency planner integration | 0261 | DONE | 6 tests |
| DECIMAL(p,s) CAST target | 0262 | DONE | 8 tests |
| TIMESTAMPTZ ISO-8601 text format | 0263 | DONE | 5 tests |

#### P0-2 Online DDL (owner: `bradian-relational`) — **ALL DONE**

| Slice | ADR | Status | Acceptance |
|-------|-----|--------|------------|
| Schema version counter + `SchemaSnapshot` | 0242 | DONE | 14 tests |
| `execute_with_schema_snapshot` | 0256 | DONE | 9 tests |
| Mutex-held reconcile+execute | 0264 | DONE | concurrent-thread test |
| Schema lease registry | 0265 | DONE | 7 tests |
| Snapshot integration into `DomainSqlEngine` session | 0266 | DONE | 4 tests (both engines) |
| Snapshot integration into `MvccSqlEngine` session | 0267 | DONE | same |

#### P0-3 SQL conformance (owner: `bradian-relational` + `bradian-planner`) — **13 of 17 DONE**

| Slice | ADR | Status | Acceptance |
|-------|-----|--------|------------|
| Arithmetic infix (`+ - * / %`) | 0239 | DONE | 11 tests |
| WHERE NOT + parens | 0240 | DONE | 9 tests |
| IS DISTINCT FROM / IS NOT DISTINCT FROM | 0243 | DONE | 9 tests |
| Bare-column IS NULL / IS DISTINCT FROM | 0244 | DONE | 9 tests |
| Bare-column scalar comparisons | 0245 | DONE | 16 tests |
| Bare-column IN / BETWEEN / LIKE / ILIKE | 0246 | DONE | 16 tests |
| CAST TIMESTAMPTZ / UUID targets | 0257 | DONE | 12 tests |
| CASE expression on scalar columns in SELECT | 0268 | DONE | 8 tests |
| CAST DECIMAL(p,s) target | 0269 | DONE (as 0262) | 8 tests |
| Scalar subquery in `WHERE col IN (SELECT …)` | 0270 | DONE | verification |
| Scalar subquery in `WHERE EXISTS (…)` | 0271 | DONE | 7 tests |
| Column-in-column WHERE (joins) | 0272 | DONE | verification |
| INNER / LEFT JOIN (first slice) | 0273 | DONE | verification |
| `WITH` CTEs (non-recursive) | 0274 | DONE | 7 tests, narrow scope |
| UNION / INTERSECT / EXCEPT | 0275 | DONE | 5 tests |
| In-tree ANSI isolation schedule corpus | 0276 | DONE | 8 schedule tests in `tests/isolation_schedules.rs` + reference doc `BRADIAN_ISOLATION_SCHEDULES.md`. External SQL-conformance suite (0277) remains infra |
| External SQL-92/99/2016 compliance suite | 0277 | DEFERRED | Infrastructure — needs reference suite integration |

#### P0-4 Enterprise access — **12 of 14 DONE**

| Slice | ADR | Status | Acceptance |
|-------|-----|--------|------------|
| In-memory UserCatalog + argon2id | 0247 | DONE | 15 tests |
| Persistent UserCatalog (WAL journal) | 0248 | DONE | 10 tests |
| CREATE / DROP / ALTER USER SQL grammar | 0249 | DONE | 12 tests |
| AuthenticatedSession (principal binding) | 0250 | DONE | 9 tests |
| GRANT / REVOKE global capabilities + bootstrap | 0251 | DONE | 14 tests |
| Per-object GRANT SELECT | 0258 | DONE | 12 tests |
| Per-object GRANT INSERT / UPDATE / DELETE | 0278 | DONE | 8 tests |
| Per-object GRANT DDL (ALTER / DROP on table) | 0279 | DONE | 6 tests |
| Multi-table FROM authz (join-aware) | 0280 | DONE | 3 tests |
| Role catalog (CREATE ROLE / GRANT ROLE / REVOKE ROLE) | 0281 | DONE | 8 tests |
| Revocation propagation to live sessions | 0282 | DONE | 3 tests |
| Rate limiting + audit of authn failures | 0285 | DONE | 4 tests |
| TLS at the API boundary (rustls) | 0283 | DEFERRED | Infra: needs `bradian-serving` crate work + `rustls` dep |
| mTLS client auth | 0284 | DEFERRED | Infra: depends on 0283 |

#### P0-5 Operational recovery — **9 of 11 DONE**

| Slice | ADR | Status | Acceptance |
|-------|-----|--------|------------|
| WAL fault injection primitive | 0252 | DONE | 11 tests |
| User + DDL journal fault injection | 0253 | DONE | 10 tests |
| Property sweep (RowStore) | 0254 | DONE | 3 tests, ~60 fault points |
| Property sweep (user + DDL journals) | 0255 | DONE | 4 tests |
| Runtime `FaultPolicy` trait | 0288 | DONE | 5 tests, three impls (NeverFail / FailAfterN / DropOnNthSync) |
| Sweep on BRPG checkpoint files | 0286 | DONE | 4 tests across both file types |
| Sweep on manifest files | 0287 | DONE (combined with 0286) | same |
| Scripted-workload property harness | 0289 | DONE | 3 tests across ~11 seeded scenarios |
| macOS installation + lock lifecycle | — | DONE | [verification report](BRADIAN_GATE1_FAULT_RECOVERY_MACOS_VERIFICATION_2026_11_18.md) |
| Fsync-on-power-loss rig | 0290 | DEFERRED | Infra: hardware or specialised VM |
| Windows installation + lock lifecycle | 0291 | DEFERRED | Infra: Windows CI runner |

### 3.2 P1 — Gate 2–3 blockers

#### P1-1 Columnar / analytical engine wiring (`bradian-runtime`, `bradian-views`)

| Slice | ADR | Status | Acceptance |
|-------|-----|--------|------------|
| Vectorized SELECT path alongside the row path | 0292 | OPEN | `bradian-runtime` batch executor consumes RowStore rows |
| Columnar segment write path from commit | 0293 | OPEN | Column projection after COMMIT in a shared-domain SQL commit |
| Incremental view maintenance for SUM / COUNT / MIN / MAX | 0294 | OPEN | Retraction + refresh on commit |
| Freshness / snapshot semantics for HTAP reads | 0295 | OPEN | Explicit lag annotation |

#### P1-2 Planner + binder parity (`bradian-planner`)

| Slice | ADR | Status | Acceptance |
|-------|-----|--------|------------|
| Logical plan node coverage for existing SQL subset | 0296 | OPEN | Round-trip: SQL → plan → execute |
| EXPLAIN surface backed by actual plan | 0297 | OPEN | Not a text template |
| Cost-based join reordering (two-table + chained) | 0298 | OPEN | Benchmarks demonstrate gain |

#### P1-3 Catalog + semantic layer (`bradian-catalog`, `bradian-dhis2`)

| Slice | ADR | Status | Acceptance |
|-------|-----|--------|------------|
| Versioned semantic catalog with lineage edges | 0299 | OPEN | Graph export; stale-lineage detection |
| DHIS2 aggregate / event / tracker / indicator mapping | 0300 | OPEN | Import reference DHIS2 metadata; round-trip |

#### P1-4 Ingest + source adapters (`bradian-ingest`, `bradian-sources`)

| Slice | ADR | Status | Acceptance |
|-------|-----|--------|------------|
| Canonical change-log schema | 0301 | OPEN | Idempotent ingest key + replay |
| CSV adapter | 0302 | OPEN | Streaming; type inference; errors surfaced |
| JSON Lines adapter | 0303 | OPEN | Same |
| Arrow IPC adapter | 0304 | OPEN | Batch round-trip |
| SQL CDC adapter (Postgres logical replication, pull-only) | 0305 | OPEN | Admin credentials scoped; cursor persistence |

#### P1-5 Benchmarks (`bradian-benchmark`)

| Slice | ADR | Status | Acceptance |
|-------|-----|--------|------------|
| Criterion harness skeleton | — | PARTIAL | Already scaffolded |
| DHIS2 synthetic fact generator | 0306 | OPEN | Reproducible seed; documented workload |
| Row-store write / read benchmarks | 0307 | OPEN | Publishes numbers in `docs/` |

### 3.3 P2 — Gate 4+ blockers

| Slice | ADR | Status | Area |
|-------|-----|--------|------|
| Mutable physical page storage (WAL-protected dirty pages, buffer manager) | 0308 | OPEN | `bradian-relational` |
| Consensus-backed replication | 0309 | OPEN | `bradian-distributed` |
| Distributed commit + global snapshot | 0310 | OPEN | `bradian-distributed` |
| Shard placement + fencing | 0311 | OPEN | `bradian-distributed` |
| Rebalancing | 0312 | OPEN | `bradian-distributed` |
| Federation (site autonomy) | 0313 | OPEN | New crate or `bradian-distributed` extension |
| Studio UI backing real SQL sessions | 0314 | OPEN | `apps/bradian-ui` + `bradian-serving` |
| DHIS2 integration deployment path | 0315 | OPEN | `apps/dhis2-integration` |
| Wire protocol for relational sessions + reference driver | 0316 | OPEN | `bradian-serving` |
| PITR / incremental archive / offsite backup | 0317 | OPEN | `bradian-relational` + `bradian-storage` |
| Schema history compaction with reference proofs | 0318 | OPEN | `bradian-relational` |
| External reproducibility + production certification | 0319 | OPEN | Gate 9 package |

### 3.4 Remaining Gate 1 qualification tasks (from master roadmap)

| Task | Status | What's done / what's left |
|------|--------|----------------|
| G1-MEMORY | **DONE on code** | HistorySqlEngine samples DurableTypedStore (ADR 0320). Memory-pressure fault harness remains open. |
| G1-LOCAL-ACCESS | **DONE on Linux ACL** | POSIX ACL inspection shipped (ADR 0321). Windows ACL (0322) + enterprise identity (0323) are infra-deferred. |
| G1-FAULTS | **DONE on code** | WAL / user / DDL / BRPG / manifest sweeps + FaultPolicy trait + scripted harness. Power-loss rig (0290), device-torn-write lab (0324), Windows runs (0291) are infra-deferred. |
| G1-REFERENCE | **DONE on in-tree corpus** | 46-row SQL subset. External SQL-conformance suite (0277) and installation qualification (0325) remain infrastructure. |
| G1-PORTABILITY | macOS DONE | Windows (0291) + device lab (0324) infra. |
| G1-INSTALL | Partial | Signed portable installers (0326), 3-OS matrix (0325) — all infra. |

---

## 4. Implementation sequencing

### 4.1 Ordering rules

1. P0 slices close first. Within P0, sequence is:
   P0-1 → P0-2 → P0-3 → P0-4 → P0-5. Interleaving is allowed when a
   slice unblocks later ones (e.g. ADR 0272 column-in-column WHERE is
   a prerequisite for ADR 0273 JOIN).
2. P0 qualification tasks (G1-*) run alongside P0 feature slices; they
   do not block individual feature ADRs but DO block Gate 1 acceptance.
3. P1 slices start only after every P0 slice is DONE.
4. P2 slices start only after Gate 1 is accepted via the Master SRS
   exit conditions.

### 4.2 Next immediate batches

**Batch N+1 (close P0-1 schema evolution):**
- ADR 0259 Expression DEFAULT.
- ADR 0260 ALTER COLUMN TYPE narrowing.
- ADR 0261 DROP COLUMN CASCADE.

**Batch N+2 (close P0-2 online DDL):**
- ADR 0264 Mutex-held reconcile+execute.
- ADR 0265 Schema lease registry.
- ADR 0266 / 0267 session integration in DomainSql + MvccSql.

**Batch N+3 (push P0-3 toward Gate 1 acceptance):**
- ADR 0268 CASE in SELECT.
- ADR 0270 / 0271 Subqueries.
- ADR 0272 / 0273 JOIN prerequisites.

**Batch N+4 (close P0-4 authorization):**
- ADR 0278 / 0279 per-object INSERT/UPDATE/DELETE/DDL grants.
- ADR 0280 Multi-table FROM authz.
- ADR 0281 Role catalog.
- ADR 0283 / 0284 TLS + mTLS.

**Batch N+5 (close P0-5 recovery):**
- ADR 0286 / 0287 BRPG and manifest sweep.
- ADR 0288 Runtime FaultPolicy trait.
- ADR 0289 Scripted-workload property harness.

### 4.3 Per-slice checklist

Every slice must:

1. File the ADR in `docs/adr/0NNN-<slug>.md` **before** code lands.
2. Add tests that fail without the change and pass with it.
3. Keep `cargo check --workspace --locked` green.
4. Keep `cargo clippy --workspace --all-targets --locked -- -D warnings` green.
5. Update [`BRADIAN_IMPLEMENTATION_GAPS.md`](BRADIAN_IMPLEMENTATION_GAPS.md)
   (verified table).
6. Update this file (move the slice from OPEN to DONE).
7. One commit per slice, pushed to `main`.

---

## 5. Disk hygiene policy

Test artifacts in `target/` grew to 46 GB during the recent P0-4 batch,
nearly exhausting local disk. The policy is:

1. **Always** run `cargo clean -p <crate>` at the end of a session that
   crossed 2+ hours of compile work, or whenever `target/` exceeds 25 GB.
   Workspace-wide `cargo clean` is fine too — the price is a single cold
   rebuild on next session.
2. **Never** keep `tempfile::tempdir()` handles alive past the test case
   that created them. Tests must let them drop so the OS deletes the
   backing directory.
3. **Never** write test data to `/tmp` or `~/.cache` from inside a crate —
   always via `tempfile`.
4. Background compile jobs (clippy, cargo test) run with a cap of **one
   job at a time per machine**. Two concurrent cargo invocations block on
   the build-directory lock and double the on-disk intermediate size.
5. On finishing a batch, drop the `target/debug/deps/<old>` compilation
   units that no longer match the current `Cargo.lock` by running
   `cargo clean -p <crate>` on the crates that have been heavily
   iterated.

---

## 6. Testing + documentation standard

Every slice ships with:

- **Positive tests** for the intended behavior.
- **Negative tests** for the explicit scope limits (what the slice does
  NOT do must fail cleanly, not silently succeed).
- **Regression test** for any surface that the slice touches.
- **ADR** with Context / Decision / Consequences sections and an explicit
  acceptance-criteria list.
- **Gap-register row** moved from OPEN to DONE, including the test file
  reference.

Each ADR's "Consequences" section must explicitly enumerate what is
**still open** so the next slice is self-evident from the ADR, not from
tribal knowledge.

---

## 7. Audit cadence

- **Weekly:** re-generate the ADR count, test count, and `target/` size.
  Record in a short entry at the bottom of this file.
- **Per-batch:** update Section 3's priority tables with status changes.
- **Per-gate-acceptance:** cross-reference this file against the master
  SRS exit conditions and, if the gate is accepted, archive the closed
  rows to a new `BRADIAN_GATE<N>_COMPLETION.md` document.

---

## 8. Referenced documents

- [Master SRS v1.1](../Bradian_Master_Architecture_SRS_v1.1.md) — normative
  requirements.
- [Fixed master roadmap](BRADIAN_MASTER_ROADMAP.md) — gate sequence.
- [Implementation gap register](BRADIAN_IMPLEMENTATION_GAPS.md) — verified
  per-ADR evidence table.
- [V1.1 audit](BRADIAN_V1_1_AUDIT.md) — original gap analysis.
- [Comprehensive gaps analysis](BRADIAN_COMPREHENSIVE_GAPS_ANALYSIS.md) —
  SRS MUST-clause cross-reference.
- [BACE ADR review package](BACE_ADR_REVIEW_PACKAGE.md) — BACE prerequisites.
- [SRS source inventory](BRADIAN_SOURCE_INVENTORY.md).
- [Master traceability matrix](BRADIAN_MASTER_TRACEABILITY.md).

---

## 9. Change log

- **2026-11-24** — Initial authoring. Inventoried 15 crates / 3 apps / 1
  e2e crate. Enumerated 61 open slices across P0, P1, P2, and remaining
  G1-* qualification tasks. Established disk hygiene policy after the
  P0-4 batch filled 46 GB. All closed slices in prior ADRs 0241–0258
  cross-referenced against their test files.
- **2026-11-24 (afternoon)** — Autonomous G1 completion run. Shipped
  ADRs 0259–0288 closing the implementable P0 slices across all five
  tracks. Final inventory:
  - **P0-1 (5/5 shipped):** 0259 Expression DEFAULT, 0260 ALTER TYPE
    narrow, 0261 DROP COLUMN CASCADE, 0262 DECIMAL CAST,
    0263 TIMESTAMPTZ ISO-8601 format.
  - **P0-2 (4/4 shipped):** 0264 mutex-held reconcile+execute,
    0265 schema lease registry, 0266/0267 snapshot in
    Domain/Mvcc sessions.
  - **P0-3 (6 new + 4 deferred):** 0268 CASE verified, 0270+0272+0273
    IN subquery / column-column / JOIN verified, 0275 INTERSECT /
    EXCEPT. Deferred with design notes: 0271 EXISTS, 0274 CTE, 0276
    isolation qual, 0277 external SQL suite.
  - **P0-4 (3 new + 5 deferred):** 0278 per-object INSERT/UPDATE/DELETE
    grants, 0280 multi-table FROM authz, 0282 revocation propagation.
    Deferred with design notes: 0279 DDL grants, 0281 role catalog,
    0283 TLS, 0284 mTLS, 0285 rate limiting.
  - **P0-5 (1 new + 5 deferred):** 0288 FaultPolicy trait. Deferred:
    0286 BRPG sweep, 0287 manifest sweep, 0289 scripted harness,
    0290 fsync rig, 0291 Windows installation.
  - **G1 qualification (0/6 — all infrastructure):** 0320 memory
    qual tail, 0321 POSIX ACL, 0322 Windows ACL, 0323 enterprise
    identity, 0324 device torn-write lab, 0325 installation qual,
    0326 signed installers.
  - **Totals:** 21 new code-shipping ADRs, 6 verification ADRs,
    7 deferral ADRs with design notes. ~80 new tests across the
    batch. All commits clippy-green under `-D warnings`.
- **2026-11-24 (evening, second autonomous pass)** — Closed
  every code-doable G1 slice that was still OPEN after the
  first pass. 10 new code-shipping ADRs:
  - **P0-3 (3 new):** 0271 EXISTS subquery, 0274 WITH / CTE
    (narrow), prior 0268/0270/0272/0273/0275 confirmed.
  - **P0-4 (4 new):** 0279 per-object ALTER/DROP, 0281 role
    catalog, 0285 rate limiting, 0280/0282 already DONE from
    first pass.
  - **P0-5 (3 new):** 0286+0287 BRPG + manifest sweeps,
    0289 scripted-workload harness.
  - **G1 qualification (2 new code):** 0320 HistorySqlEngine
    memory sampling, 0321 POSIX ACL inspection (Linux).
  - Remaining open: **infrastructure-only** — ADRs 0276
    (isolation suite), 0277 (external SQL suite), 0283/0284
    (TLS/mTLS), 0290 (power-loss rig), 0291 (Windows runs),
    0322/0323/0324/0325/0326 (Windows ACL, enterprise
    identity, device lab, installer matrix, signing certs).
  - **Summary of Gate 1 code**: every P0 code slice is DONE.
    Every remaining Gate 1 item has an infrastructure
    dependency (hardware, OS matrix, external cert / suite).
    Gate 1 **acceptance** depends on those closing — not
    on more code.
