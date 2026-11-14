# Bradian comprehensive implementation plan

Working document, 14 November 2026. Scope: every normative Bradian requirement
in [Master SRS v1.1](../Bradian_Master_Architecture_SRS_v1.1.md) and the
subordinate ADRs/operations/security docs in `docs/`. The inherited DER /
DHIS2 analytical specification is explicitly outside this plan; it stays as
an adapter.

This plan is a **map, not a commitment calendar**. Sequencing follows the
fixed master gate order; every task is sized small enough to be a single
vertical commit with an ADR, tests and verification per
[CLAUDE.md §4 and §5](../../CLAUDE.md). Items that require owner input
(design freeze, hardware, external dependency) are called out explicitly;
they are **not** auto-startable.

## 1. Guiding rules (non-negotiable)

Copied here so every reader sees them before picking up a task:

1. **One vertical slice per commit.** Implementation + tests + ADR +
   verification report, or it is not done. No scaffold-only PRs.
2. **No persistent format or wire protocol freeze without an ADR** and,
   where cross-process trust is involved, threat-model review.
3. **Fail closed.** Unknown input, unknown platform, unknown identity →
   explicit refusal, never silent approximation.
4. **Honest status.** Verification reports must name what was and was NOT
   executed on the slice. Platform/hardware claims need platform/hardware
   runs.
5. **No compatibility shims for unused code.** Deletion is preferred over
   `#[cfg(todo)]`. New flags only when behavior would otherwise regress.
6. **Legacy DAP identifiers (`dap-*`, `.derc`, `apps/dap-ui/`) stay.**
   Bulk rename is not in scope until a migration + compatibility suite
   exists. Visible branding is Bradian Database Systems / Bradian Studio.

## 2. Current state snapshot (14 Nov 2026)

Baseline commit `c309f0f`. 823 workspace tests, 0 failures.

| Master gate | Status | Earliest remaining blocker |
|---|---|---|
| 0 Evidence, risk, specification closure | Reviewable baseline; some design items remain unfrozen | Owner sign-off on authorization wire format, cluster identity, distributed timestamp |
| 1 Portable kernel + single-node transaction | Active; shipped G1-MEMORY (×5), G1-FAULTS (×1), G1-LOCAL-ACCESS (×1), G1-REFERENCE (×1) code-only slices | Hardware fault lab, cross-platform runs, owner-UID / ACL design, external SQL reference |
| 2 Relational compatibility and safe schema lifecycle | Not accepted | Full schema evolution design; concurrent-online-DDL protocol |
| 3 Columnar engine + commit-consistent HTAP | Not accepted | Authoritative column projection from transactional state |
| 4 Single-cluster replication + safe placement | Blocked | Secure identity, consensus choice, placement contract |
| 5 Distributed commit + global snapshot correctness | Blocked | Global-ordering prototype and consensus acceptance |
| 6 Studio, durable operations, migration | Not accepted | Studio design, operator workflow, versioned migration framework |
| 7 Federation + site autonomy | Blocked | Installation identity contract, delta/conflict semantics |
| 8 Cluster-consistent recovery + hardening | Blocked | Complete trust/storage/replication design |
| 9 Production qualification + external reproducibility | Not accepted | Benchmark harness, cross-platform CI, published artifacts |

## 3. Gate 1 — remaining tasks (active)

Gate 1 is the only currently-active master gate. The shipped slices close
measurable portions of G1-MEMORY, G1-FAULTS, G1-LOCAL-ACCESS and
G1-REFERENCE but none of those stable tasks has reached its full exit bar.

### 3.1 G1-MEMORY — remaining decoded/compat/global budgets

| Task | Scope | Depends on | Risk |
|---|---|---|---|
| Lifecycle/domain directory listing admission | Extend ADR 0051 pattern to `lifecycle.rs`, `history_migration.rs`, `mvcc_sql.rs` discovery paths | — | Low; same pattern as shipped |
| Decoded domain and lifecycle state admission | `DomainLimits`, `LifecycleLimits` with per-domain decoded entry caps | ADR 0054 shape | Low-medium |
| Cross-domain transaction totals | Aggregate `TransactionLimits` across catalog+domain+history | ADR 0050 | Medium — must preserve existing per-domain caps |
| Alternate engine admission | Shared admission through `durable_mvcc`, `typed_mvcc`, `mvcc_sql` | ADRs 0025–0027 | Medium |
| Global process budget | Process-level RSS/arena ceiling with graceful refuse | All of above | **Needs owner decision**: pure accounting or OS-level `cgroups`/`ulimit` integration |

Exit bar: the operations guides in `docs/operations/` cover every
decoded-state source; a quota refusal in any one of them preserves
state and is distinct from corruption/IO.

### 3.2 G1-FAULTS — remaining fault matrix

| Task | Scope | Depends on | Risk |
|---|---|---|---|
| Deterministic rename/sync-failure injection | Introduce `storage::io` trait wrapper only on named hot paths (WAL append, snapshot rename, manifest publish); feature-gated test harness implements injection | New ADR 0056 | **Medium design** — the trait must not slow the hot path; keep it zero-cost in release |
| Real ENOSPC / EIO coverage | Loopback filesystem image (`tmpfs` size limit on Linux) + bash harness invoked from a `#[ignore]` test | Fault-injection trait | Medium — Linux-only; needs reproducible make/script |
| Device-level torn-write coverage | Needs physical rig | — | **Hardware blocker** |
| Windows / macOS recovery runs | Needs cross-platform CI | — | **Infrastructure blocker** |

Exit bar: every WAL/manifest/checkpoint write site fails deterministically
under injected error with a documented recovery outcome; real ENOSPC is
proven on Linux; cross-platform is tracked as its own milestone.

### 3.3 G1-LOCAL-ACCESS — remaining identity work

| Task | Scope | Depends on | Risk |
|---|---|---|---|
| Effective-UID ownership check | Extend `local_access::ensure_private_root` to require `metadata.uid() == geteuid()`; test against test-only override | ADR 0053 + new ADR 0057 | Low — but needs a deterministic test injection for `uid` |
| Parent-path trust | Reject world-writable or symlinked ancestors; canonicalize once and compare components | Owner decision between canonicalize-once vs per-component | **Needs owner decision** |
| POSIX ACL inspection | Read extended attributes; refuse non-empty ACL grants beyond owner | Platform-specific `libacl` or `acl_get_file` FFI | **Dependency decision** |
| Windows ACL / SID | Map SID + DACL rules to private-root invariants | Separate Windows-only module | **Infrastructure blocker** |
| Enterprise identity (SSO/mTLS binding) | Session-level mapping of operating-system principal to Bradian role | Depends on G6 Studio auth design | **Needs owner decision** |

Exit bar: hostile filesystems (world-writable parent, UID squat, ACL
grant, symlinked ancestor) all refuse before any SQL executes;
Windows is tracked as its own cross-platform milestone.

### 3.4 G1-REFERENCE — remaining reference work

| Task | Scope | Depends on | Risk |
|---|---|---|---|
| Expand `BRADIAN_SQL_SUBSET.md` to cover DESCRIBE, RELEASE SAVEPOINT, INSERT DEFAULT, UPDATE expression boundary, TimestampTz/Uuid comparison | Add new corpus rows + tests | ADR 0055 | Low; purely additive |
| External SQL reference suite | Choose and adapt a public suite (e.g. SQL Logic Test fragments). Each run produces a checked diff against our subset | Suite selection | **Needs owner decision** on which external corpus (SQL-92 / SQLite logic / others) |
| Documented isolation-level qualification | Formal statement of what snapshot and read-validation modes do and do not guarantee; linked to the Jepsen-style schedules already in the test suite | — | Low but writing-heavy |
| Cross-platform installer + lock lifecycle tests | Windows and macOS installer + lock recovery runs | Cross-platform CI | **Infrastructure blocker** |

### 3.5 G1-INSTALL, G1-PORTABILITY

Both remain marked Partial / Open. Code is Linux-only in practice.
Portable release installers, code signing, upgrade/downgrade paths and the
Windows/macOS lock lifecycle are all pending the cross-platform CI
infrastructure decision.

## 4. Gate 2 — relational compatibility and safe schema lifecycle

Not accepted. Current shipped subset covers additive DDL via G23/G35/G36
and bounded UPDATE expressions via G43. The master bar is much broader.

### 4.1 Broader schema evolution

| Task | New ADRs | Precondition |
|---|---|---|
| `SET DEFAULT` / `DROP DEFAULT` on existing columns | ADR 0056 (default-expression semantics and recovery) | Compatibility rule: default change must not rewrite old rows |
| `SET NOT NULL` with checked backfill | ADR 0057 (bounded backfill protocol) | Needs `online-DDL` design to avoid full-table lock |
| `DROP COLUMN` | ADR 0058 (tombstone + row re-codec on compact) | Needs compaction design (G2-STORAGE) |
| `RENAME COLUMN` | ADR 0059 (name/identity split, catalog dual-name window) | Must preserve query-time name resolution without rewriting rows |
| Type change (narrowing, widening) | ADR 0060 (checked cast + bounded rejection) | Narrowing must refuse before publication; widening requires row re-encoding |
| Composite / secondary index DDL (`CREATE INDEX`, `DROP INDEX`) | ADR 0061 | Needs mutable physical storage (G3) before it is useful |

### 4.2 Concurrent online DDL

| Task | New ADRs | Precondition |
|---|---|---|
| Reader schema leases | ADR 0062 (versioned schema handle + release on commit/rollback/drop) | Catalog revision chain already exists; this builds on it |
| Writer fencing across exclusive ALTER | ADR 0063 (exclusive mutable ownership vs reader leases) | Interacts with MVCC version retention |
| Explicit old-writer commit semantics | ADR 0064 (what happens when a writer holds a stale schema pin at commit) | ADR 0062 |

Exit: existing bounded single-process ALTER paths extended so that a
reader with a live lease can finish its transaction at the old schema
while a new ALTER is in progress; a commit at the stale schema must either
succeed if additive or fail closed with a precise error otherwise.

### 4.3 SQL conformance expansion (incremental, feature-flagged)

Every feature below ships as one or more slices with its own ADR. Each
also expands `BRADIAN_SQL_SUBSET.md`.

- Join: inner, left outer, right outer, cross. Starts with INNER JOIN on
  a single equality predicate and no plan choice; no hash/merge physics
  until planner work lands in Gate 3.
- Range predicates (`<`, `>`, `BETWEEN`, `IN` with literal lists).
- `OR` in `WHERE`; two-rail execution in the planner.
- `GROUP BY` with COUNT / SUM / MIN / MAX / AVG, scalar-only.
- `ORDER BY` for non-history SELECT.
- `HAVING`.
- `DISTINCT`.
- `UNION ALL` then `UNION`.
- Column aliases (`SELECT col AS x`); table aliases.
- Scalar sub-queries in `WHERE` (not correlated initially).
- Correlated sub-queries.
- CTEs (`WITH`).
- Prepared statements (reuse of parse+bind).
- Secondary and foreign-key enforcement (parse already exists; enforcement
  requires index infrastructure).

Each item above is a multi-slice workstream. Full SQL-92 conformance is
not a single slice; the honest unit is "+N statements in the corpus,
+X tests, +Y rejection cases removed".

### 4.4 Full isolation qualification

| Task | Depends on |
|---|---|
| Serializable under opt-in read validation (expand to range locks) | ADR 0037, read-validation observations |
| Repeatable read | Snapshot already gives this; needs documented statement |
| Documented anomaly map | Each anomaly (dirty read, non-repeatable read, phantom, write skew) mapped to mode and proven by a schedule |

Exit: the documented isolation levels are covered by regression schedules
and `BRADIAN_SQL_SUBSET.md` lists which modes satisfy which guarantees.

## 5. Gate 3 — columnar engine + commit-consistent HTAP

Not accepted. The inherited analytical engine exists under `dap-*`
crates; it is not integrated with the transactional state and cannot
produce commit-consistent column projections.

### 5.1 Mutable physical storage (prerequisite)

Everything in Gate 3 and later assumes we can write to pages in place.
Today the B+tree is immutable per generation.

| Task | New ADRs | Risk |
|---|---|---|
| Dirty-page WAL protocol | ADR 0065 (dirty-page WAL frames, buffer-pool eviction) | **High** — longest single workstream in the plan |
| Buffer-pool writes | ADR 0066 (clock-pro / LRU-K choice, pinning) | High |
| In-place B+tree mutation | ADR 0067 (page split/merge under WAL protection) | High |
| Checkpoint under mutable pages | ADR 0068 (fuzzy vs consistent checkpoints) | High |
| Crash recovery of dirty-page WAL | ADR 0069 (ARIES-style redo/undo or MVCC-only choice) | High |

Exit: every existing test still passes with the storage engine running
in mutable mode. This is a months-long effort; it should land with
feature-flagged co-existence until mature.

### 5.2 Commit-consistent column projection

| Task | New ADRs | Depends on |
|---|---|---|
| Transactional watermark for columnar segments | ADR 0070 (`committed_watermark`, `last_projected_watermark`) | Mutable storage |
| Row → column projector service (in-process) | ADR 0071 | Watermark design |
| Freshness / bounded-staleness query contract | ADR 0072 | Watermark |
| OLTP resource protection during analytics | ADR 0073 (scan admission across row and column stores) | — |

### 5.3 Analytical planner integration

The existing planner in `dap-planner` is scoped to the analytical engine.
Integration tasks:

- Unified binder that recognises both transactional and columnar tables.
- Join algorithms (hash, merge) implemented against commit-consistent
  snapshots.
- EXPLAIN output that cites both engines.

## 6. Gate 4 — single-cluster replication + safe placement

Blocked until Gate 3 is at least feature-complete locally.

### 6.1 Secure identity and transport

| Task | New ADRs |
|---|---|
| Node identity (signing key, long-lived cert) | ADR 0074 |
| mTLS node-to-node | ADR 0075 (TLS 1.3, allowed cipher suites, key rotation) |
| Replication wire format v1 | ADR 0076 |
| Bootstrap and trust-on-first-use alternative | ADR 0077 |

**Needs owner decision** before any of these ADRs is drafted: whether
keys are provisioned via an external PKI or self-generated with a shared
secret.

### 6.2 Consensus and placement

| Task | New ADRs |
|---|---|
| Consensus choice (Raft vs multi-leader) | ADR 0078 |
| Group membership and fencing | ADR 0079 |
| Shard placement contract | ADR 0080 |

## 7. Gate 5 — distributed commit + global snapshot correctness

Blocked. Depends on Gate 4 and on a global-ordering prototype.

| Task | New ADRs |
|---|---|
| Distributed timestamp source (HLC vs TrueTime-style) | ADR 0081 |
| Distributed commit protocol (2PC with presumed-abort) | ADR 0082 |
| Global snapshot isolation | ADR 0083 |
| Read-only quorum snapshots | ADR 0084 |

## 8. Gate 6 — Studio, durable operations, migration

Not accepted.

### 8.1 Bradian Studio (web UI)

The existing `apps/dap-ui/` is a React scaffold with a shell, not a
production UI. The SRS §11 enumerates: Server Overview, Database Explorer,
Table Designer, SQL Workbench, Cluster, Security, Audit, Monitoring, Jobs.

| Workstream | New ADRs |
|---|---|
| Studio authentication (replaces bootstrap token) | ADR 0085 (session cookie + CSRF + rotation) |
| Studio authorization (per-user, per-object) | ADR 0086 |
| Streaming result set for SQL Workbench | ADR 0087 |
| Query cancellation surface | ADR 0088 |
| Table designer DDL emission | ADR 0089 |
| Monitoring + audit views | ADR 0090 |

**Needs owner decision**: ship Studio initially as a local-only workbench
(signed-in via OS user) or wait until enterprise identity (ADR 0085 et al)
is accepted.

### 8.2 Operator migration framework

| Task | New ADRs |
|---|---|
| Versioned on-disk compatibility registry | ADR 0091 |
| Zero-downtime migration playbooks | ADR 0092 |
| Automated migration dry-run | ADR 0093 |

### 8.3 Operational recovery qualification

| Task |
|---|
| Power-loss drills (requires hardware lab) |
| Long-running restore rehearsals |
| Backup compaction and PITR validation |

Hardware labs remain a blocker for the final qualification runs.

## 9. Gate 7 — federation + site autonomy

Blocked.

| Task | New ADRs |
|---|---|
| Installation identity | ADR 0094 |
| Delta exchange protocol | ADR 0095 |
| Conflict and partition semantics | ADR 0096 |
| Offline-first edge profile integration (reuses DER adapter) | ADR 0097 |

## 10. Gate 8 — cluster-consistent recovery + hardening

Blocked on Gates 4, 5 and 7.

| Task | New ADRs |
|---|---|
| Cluster-consistent backup manifest | ADR 0098 |
| Incremental / PITR archives | ADR 0099 |
| Remote archive encryption | ADR 0100 |
| Key management and rotation | ADR 0101 |

## 11. Gate 9 — production qualification and external reproducibility

Not accepted.

| Task |
|---|
| Reproducible benchmark harness (`dap-benchmark` + published results) |
| Workload corpora with documented hardware baselines |
| Public reproduction kit |
| Cross-platform release pipeline |
| External conformance runs |

## 12. Cross-cutting workstreams

These do not map to one gate but touch many.

### 12.1 BACE adaptive compression

[BACE SRS v1.0](../BACE_Comprehensive_Requirements_SRS_v1.0.md) is accepted
as a target. Not integrated. The integration touches WAL, checkpoint,
backup, replication and federation. It should follow Gate 3 mutable
storage; introducing BACE earlier risks encoding churn.

### 12.2 Driver / wire protocol

The SRS mentions drivers. No wire protocol exists. A driver workstream
depends on:

1. Enterprise identity (ADR 0085 et al).
2. A frozen session protocol (**needs owner decision**: PostgreSQL wire
   compatibility vs native binary protocol).

### 12.3 DER / DHIS2 adapter

Retained as an inherited adapter. Not part of the Bradian normative scope.
Tasks: keep DHIS2 wire endpoints compatible until a versioned replacement
is published; migrate dap-ui identifiers once Studio ships.

### 12.4 CI and release

Currently manual only, by owner instruction. Any change here requires
owner acknowledgement because CLAUDE.md records the standing policy.

## 13. Design decisions that must be owner-signed before implementation

These are the items where starting implementation without a decision would
force a later rewrite or commit us to a format that may not reflect intent.

| Decision | Blocks |
|---|---|
| Global process memory accounting model | G1-MEMORY global budget |
| Fault-injection design: trait wrapper vs filesystem shim | G1-FAULTS deterministic matrix |
| Parent-path trust policy (canonicalize-once vs per-component) | G1-LOCAL-ACCESS parent trust |
| ACL inspection platform binding | G1-LOCAL-ACCESS ACL |
| External SQL reference corpus choice | G1-REFERENCE external suite |
| Online-DDL protocol (ADR 0062–0064) | Gate 2 schema evolution |
| Mutable storage protocol (ADR 0065–0069) | Gate 3 HTAP |
| PKI vs self-generated identities | Gate 4 TLS/mTLS |
| Consensus choice (Raft or alternatives) | Gate 4 replication |
| Global-timestamp source (HLC vs TrueTime-style) | Gate 5 distributed commit |
| Studio auth posture (local-only now vs wait for enterprise identity) | Gate 6 Studio |
| Driver wire protocol (PostgreSQL-compatible or native) | Driver workstream |

None of these is code-only work; they are all the kind of decision the
Bradian threat model and master SRS call out as design blockers.

## 14. What this plan deliberately does not promise

- A delivery calendar. The plan is ordered, not scheduled. Schedules
  belong in a separate project tracker and must reflect team capacity.
- Production acceptance of any gate beyond its current honest status. No
  gate can be marked accepted without its exit evidence; this plan does
  not short-circuit that.
- Removal of the inherited `dap-*` identifiers or `.derc` format before a
  versioned migration is designed.
- CI automation. The owner instruction makes CI manual-only; this plan
  does not change that.
- Hardware qualification. Power-loss, torn-write and cross-platform runs
  need labs this plan does not procure.

## 15. How to use this plan

For each task in §3–§12:

1. Check `docs/implementation/BRADIAN_IMPLEMENTATION_GAPS.md` for the
   current status. The gap register is authoritative; this plan is the
   decomposition.
2. Pick the smallest unstarted task whose preconditions are met and
   whose design is decided (§13).
3. Open the referenced ADR number; if it does not exist, draft it in
   `docs/adr/` **before** writing code.
4. Implement, test, write the verification report, update the gap
   register and roadmap in the same commit.
5. Push to `main` per the standing authorization (CLAUDE.md §5).

Status update rhythm: this document should be revised when a gate task
moves between Open → Partial → Complete, or when a design decision in
§13 is signed off.

## 16. Open questions for the owner

Collected from the plan above, in priority order:

1. Memory accounting model — pure counting, or hook into the OS
   (cgroups/jobobjects)?
2. Fault-injection architecture — are we willing to carry a trait
   wrapper on the hot path even if feature-gated?
3. External SQL reference suite — which one?
4. Online-DDL protocol — can we commit to the reader-lease model, or do
   we want a different concurrency story?
5. Mutable storage go-ahead — is now the right time to start the
   months-long dirty-page project?
6. Identity provisioning — is there an external PKI we must integrate
   with, or do we ship a Bradian-native CA?
7. Consensus choice — Raft, or something else?
8. Studio posture — local-only workbench first, or wait for enterprise
   identity?
9. Driver — PostgreSQL wire-compatible or native?

Answers to these unblock the largest remaining workstreams.
