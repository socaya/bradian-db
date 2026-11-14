# Bradian — completed work (14 November 2026 session)

Baseline before session: commit `b3e22f4` (main).
Session end: commit `3338afc` (main).
Branch: `main` (every commit pushed upstream).
Workspace tests: **772 → 855** (+83 tests, 0 failures throughout).
Clippy clean on every touched crate.
Roadmap verifier (`scripts/verify_roadmap.py`) passes.

## 1. Commit log (chronological, this session only)

| # | Commit | Type | One-line scope |
|---|---|---|---|
| 1 | `304d783` | feat | G1-MEMORY: bound directory listings on row-store and legacy migration (ADR 0051) |
| 2 | `dd78b4a` | test | G1-FAULTS: add deterministic row-store fault recovery matrix (ADR 0052) |
| 3 | `b664504` | feat | G1-LOCAL-ACCESS: share POSIX private-root invariants via `bradian_relational::local_access` (ADR 0053) |
| 4 | `3dcdd23` | feat | G1-MEMORY: bound decoded persistent-catalog state and journal events (ADR 0054) |
| 5 | `c309f0f` | feat | G1-REFERENCE: publish documented SQL subset corpus with executable claims (ADR 0055) |
| 6 | `93fd9a5` | docs | Add comprehensive implementation plan (`BRADIAN_COMPREHENSIVE_IMPLEMENTATION_PLAN.md`) |
| 7 | `77f6bf7` | feat | G1-MEMORY: bound decoded multi-table-domain state (ADR 0056) |
| 8 | `76f89c5` | test | G1-REFERENCE: extend SQL subset corpus with 7 new claims |
| 9 | `46b2ec1` | docs | Publish written statement of Bradian isolation levels (`BRADIAN_ISOLATION_LEVELS.md`) |
| 10 | `eb159ce` | test | G1-FAULTS: extend fault recovery matrix with 3 new cases |
| 11 | `55db0cb` | feat | G1-LOCAL-ACCESS: enforce effective-UID ownership on private-root (ADR 0057) |
| 12 | `67bc9e1` | feat | G1-MEMORY: bound in-memory MvccStore version retention (ADR 0058) |
| 13 | `50f157e` | test | G1-REFERENCE: exercise 128-node UPDATE expression boundary |
| 14 | `80eb4bb` | docs | Scrub SQL subset reference: TimestampTz/Uuid are not SQL-reachable |
| 15 | `6fb2a42` | docs | Index operations guides and cite missing ADRs |
| 16 | `888371d` | docs | Dedicated operator guides for ADRs 0051/0054/0056/0058 |
| 17 | `f5c0f6c` | feat | G1-MEMORY: propagate MvccStoreLimits and CatalogLimits through alternate engines (TypedMvccStore + MvccSqlEngine `open_with_limits`) |
| 18 | `5a0cb2d` | docs | Publish `bradian-relational` module review |
| 19 | `a94aefa` | docs | Comprehensive gaps analysis cross-checked against SRS v1.1 and BACE |
| 20 | `3338afc` | feat | G1-MEMORY: propagate DomainLimits and CatalogLimits through DomainSqlEngine |

## 2. New ADRs

| ADR | Title | Scope |
|---|---|---|
| [0051](docs/adr/0051-bounded-directory-listings.md) | Bounded directory listings | Per-listing `DirectoryLimits`; row-store + legacy-migration call sites |
| [0052](docs/adr/0052-deterministic-fault-recovery.md) | Deterministic row-store fault recovery | Six crash outcomes exercised by real-filesystem tests |
| [0053](docs/adr/0053-private-root-local-access.md) | Private-root local access policy | POSIX symlink / mode-0o700 / NotADirectory invariants shared across CLI and library |
| [0054](docs/adr/0054-decoded-catalog-admission.md) | Decoded persistent-catalog admission | `CatalogLimits` + event-byte cap |
| [0055](docs/adr/0055-sql-subset-conformance-corpus.md) | Documented SQL subset conformance corpus | Positive + negative claims, each backed by a passing test |
| [0056](docs/adr/0056-decoded-domain-admission.md) | Decoded multi-table-domain admission | `DomainLimits` on `MultiTableStore` |
| [0057](docs/adr/0057-private-root-effective-uid.md) | Effective-UID private-root check | Amends ADR 0053; adds `WrongOwner` variant + `libc::geteuid` |
| [0058](docs/adr/0058-mvcc-store-admission.md) | Opt-in `MvccStore` version admission | `MvccStoreLimits`; amendment covers propagation through `TypedMvccStore` / `MvccSqlEngine` / `DomainSqlEngine` |

## 3. New reference and planning documents

| Path | Role |
|---|---|
| [docs/implementation/BRADIAN_COMPREHENSIVE_IMPLEMENTATION_PLAN.md](docs/implementation/BRADIAN_COMPREHENSIVE_IMPLEMENTATION_PLAN.md) | Gate-by-gate plan: scope, preconditions, ADR numbers, explicit owner-decision blockers (§13), nine open owner questions (§16) |
| [docs/implementation/BRADIAN_MODULE_REVIEW.md](docs/implementation/BRADIAN_MODULE_REVIEW.md) | Per-module audit of 35 source modules; test-to-module mapping; cross-cutting observations |
| [docs/implementation/BRADIAN_COMPREHENSIVE_GAPS_ANALYSIS.md](docs/implementation/BRADIAN_COMPREHENSIVE_GAPS_ANALYSIS.md) | SRS v1.1 + BACE SRS cross-check; separates invariant MUST (release-blocking) from feature shortfalls (disclosure-acceptable); priority matrix P0–P4 |
| [docs/reference/BRADIAN_SQL_SUBSET.md](docs/reference/BRADIAN_SQL_SUBSET.md) | Positive/negative SQL conformance corpus, every row cites a passing test |
| [docs/reference/BRADIAN_ISOLATION_LEVELS.md](docs/reference/BRADIAN_ISOLATION_LEVELS.md) | Anomaly-by-anomaly behaviour statement with 12 cited tests; §7 lists explicit non-claims (SERIALIZABLE, distributed) |
| [docs/operations/README.md](docs/operations/README.md) | Operations guide index enumerating 15 admission policies with ADR citations |
| [docs/operations/DIRECTORY_LISTING_LIMITS.md](docs/operations/DIRECTORY_LISTING_LIMITS.md) | Operator guide for ADR 0051 |
| [docs/operations/CATALOG_LIMITS.md](docs/operations/CATALOG_LIMITS.md) | Operator guide for ADR 0054 |
| [docs/operations/DOMAIN_LIMITS.md](docs/operations/DOMAIN_LIMITS.md) | Operator guide for ADR 0056 |
| [docs/operations/MVCC_STORE_LIMITS.md](docs/operations/MVCC_STORE_LIMITS.md) | Operator guide for ADR 0058 |

## 4. Verification reports (one per code-shipping slice)

Every verification report lists what was executed and what was NOT executed on its slice.

| Path | Baseline | Scope |
|---|---|---|
| `BRADIAN_GATE1_DIRECTORY_LIMITS_VERIFICATION_2026_11_14.md` | `b3e22f4` | Slice 1 — directory admission |
| `BRADIAN_GATE1_FAULT_RECOVERY_VERIFICATION_2026_11_14.md` | `304d783` | Slice 2 — initial fault matrix |
| `BRADIAN_GATE1_LOCAL_ACCESS_VERIFICATION_2026_11_14.md` | `dd78b4a` | Slice 3 — private-root helper |
| `BRADIAN_GATE1_CATALOG_LIMITS_VERIFICATION_2026_11_14.md` | `b664504` | Slice 4 — catalog admission |
| `BRADIAN_GATE1_SQL_SUBSET_CORPUS_VERIFICATION_2026_11_14.md` | `3dcdd23` | Slice 5 — SQL subset corpus |
| `BRADIAN_GATE1_DOMAIN_LIMITS_VERIFICATION_2026_11_14.md` | `93fd9a5` | Slice B — domain admission |
| `BRADIAN_GATE1_SQL_SUBSET_EXTENSION_VERIFICATION_2026_11_14.md` | `77f6bf7` | Slice C — corpus extension |
| `BRADIAN_GATE1_ISOLATION_LEVELS_VERIFICATION_2026_11_14.md` | `76f89c5` | Slice D — isolation statement |
| `BRADIAN_GATE1_FAULT_RECOVERY_EXTENSION_VERIFICATION_2026_11_14.md` | `46b2ec1` | Slice E — fault matrix extension |
| `BRADIAN_GATE1_LOCAL_ACCESS_UID_VERIFICATION_2026_11_14.md` | `eb159ce` | Slice F — effective-UID check |
| `BRADIAN_GATE1_MVCC_LIMITS_VERIFICATION_2026_11_14.md` | `55db0cb` | Slice G — MvccStore admission |
| `BRADIAN_GATE1_EXPRESSION_NODE_BOUNDARY_VERIFICATION_2026_11_14.md` | `67bc9e1` | Slice H — 128-node boundary |
| `BRADIAN_GATE1_TIMESTAMPTZ_UUID_SCRUB_VERIFICATION_2026_11_14.md` | `50f157e` | Slice I — SQL-reachability scrub |
| `BRADIAN_GATE1_OPERATIONS_INDEX_VERIFICATION_2026_11_14.md` | `80eb4bb` | Slice J — operations index |
| `BRADIAN_GATE1_OPERATIONS_GUIDES_VERIFICATION_2026_11_14.md` | `6fb2a42` | Slice K — four operator guides |
| `BRADIAN_GATE1_TRANSITIVE_ADMISSION_VERIFICATION_2026_11_14.md` | `888371d` | Slice L — alternate-engine propagation |
| `BRADIAN_GATE1_DOMAIN_SQL_LIMITS_VERIFICATION_2026_11_14.md` | `a94aefa` | Slice M — DomainSqlEngine propagation |

All reports live in `docs/implementation/`.

## 5. New test files

| Path | Added by | Count |
|---|---|---|
| `crates/bradian-relational/tests/directory_limits.rs` | Slice 1 | 5 |
| `crates/bradian-relational/tests/fault_recovery.rs` | Slices 2 + E | 9 |
| `crates/bradian-relational/tests/catalog_limits.rs` | Slice 4 | 5 |
| `crates/bradian-relational/tests/sql_subset_corpus.rs` | Slices 5 + C + H + I | 36 |
| `crates/bradian-relational/tests/domain_limits.rs` | Slice B | 4 |
| `crates/bradian-relational/tests/mvcc_limits.rs` | Slice G | 5 |
| `crates/bradian-relational/tests/transitive_admission.rs` | Slices L + M | 7 |

Plus 2 new private-root tests appended to the `local_access` unit module (Slice F).

## 6. New library surface

### `runtime_limits.rs` — new admission types

| Type | Default | Introduced in |
|---|---|---|
| `DirectoryLimits { max_entries, max_name_bytes }` | 65_536 / 4 MiB | Slice 1 |
| `CatalogLimits { max_databases, max_schemas, max_tables, max_revisions, max_event_bytes }` | 1_024 / 4_096 / 65_536 / 1_024 / 64 KiB | Slice 4 |
| `CatalogUsage` | — | Slice 4 |
| `DomainLimits { max_tables }` | 65_536 | Slice B |
| `MvccStoreLimits { max_distinct_keys, max_total_versions }` | 1_000_000 / 4_000_000 | Slice G |

### New `ResourceError` variants

- `DirectoryLimit { resource, limit }` (Slice 1)
- `CatalogLimit { resource, limit }` (Slice 4)
- `DomainLimit { resource, limit }` (Slice B)
- `MvccLimit { resource, limit }` (Slice G)

### New open / configuration entry points

| Entry point | Introduced in |
|---|---|
| `RowStore::open_with_policies(root, StorageLimits)` carries `directory` field | Slice 1 |
| `legacy_migration::migrate_legacy_sql_with_policies(src, dst, DirectoryLimits)` | Slice 1 |
| `PersistentCatalog::open_with_limits(path, CatalogLimits)` + `resource_limits()` + `resource_usage()` | Slice 4 |
| `MultiTableStore::open_with_limits(root, DomainLimits)` + `resource_limits()` + `registered_table_count()` | Slice B |
| `MvccStore::open_with_limits(root, MvccStoreLimits)` + `resource_limits()` | Slice G |
| `TypedMvccStore::open_with_limits(root, catalog_tables, MvccStoreLimits)` | Slice L |
| `MvccSqlEngine::open_with_limits(root, MvccStoreLimits, CatalogLimits)` | Slice L |
| `DomainSqlEngine::open_with_limits(root, DomainLimits, CatalogLimits)` | Slice M |

### New `local_access` module

- `PrivateRootPolicy::{RequireExisting, CreateIfMissing}` (Slice 3)
- `LocalAccessError::{Unsupported, NotFound, NotADirectory, SymlinkedRoot, LooseMode{mode}, WrongOwner{owner_uid, expected_uid}, Io}` (Slices 3 + F)
- `ensure_private_root(path, policy)` + `ensure_private_root_owned_by(path, policy, expected_uid)` (Slices 3 + F)
- Crate-level `#![forbid(unsafe_code)]` relaxed to `#![deny(unsafe_code)]`; the single `libc::geteuid()` call uses `#[allow(unsafe_code)]` with a `SAFETY:` comment (ADR 0057)

## 7. Honest limits recorded

Each slice's verification report names scope NOT closed. Consolidated from the gaps analysis, the following remain explicitly open:

### 7.1 Scoped deviations from SRS v1.1 (documented, not defects)

- Immutable B+tree generations vs SRS BRD-STO-005 (ADRs 0007/0010/0012; plan §5.1)
- No FK / UNIQUE / CHECK enforcement vs SRS BRD-REL-002 (ADR 0009; `BRADIAN_SQL_SUBSET.md`)
- Snapshot + bounded read-validation vs SRS BRD-TXN-002 SERIALIZABLE (ADR 0037; `BRADIAN_ISOLATION_LEVELS.md` §7)
- Exclusive ALTER only vs SRS BRD-DDL-001 online DDL (ADRs 0031/0036; plan §4.2)
- `TimestampTz` / `Uuid` unreachable from SQL vs SRS BRD-REL-004 (Slice I test)
- Offline-only migration vs SRS BRD-MIG-002 CDC (ADRs 0017/0022/0025)
- Local-process only vs every distributed SRS clause
- POSIX-only `local_access` vs SRS BRD-PORT-001 (ADR 0053)
- Append-only catalog journal vs SRS BRD-REL-001 (ADR 0016; plan §6.1)

### 7.2 Blocked on owner decisions (§13 of the implementation plan)

Twelve items require signed ADRs before implementation. Summary:

| Decision | Blocks |
|---|---|
| Global memory accounting model | G1-MEMORY global budget, BRD-OPS-007 |
| Fault-injection design | G1-FAULTS real-ENOSPC |
| External SQL reference corpus choice | BRD-SQL-005/006 |
| Parent-path trust policy | INV-003 |
| ACL inspection platform binding | G1-LOCAL-ACCESS ACL |
| Online-DDL protocol | BRD-DDL-001 |
| Mutable storage protocol | BRD-STO-005, HTAP |
| PKI vs self-generated identities | BRD-SEC-001/003, Gate 4 |
| Consensus choice | BRD-CL-*, Gate 4 |
| Global-timestamp source | BRD-DTX-*, Gate 5 |
| Studio auth posture | BRD-STU-* |
| Driver wire protocol | BRD-API-001 |

Plus four BACE-specific owner decisions: central policy service, codec registry capability negotiation, BRPG v2 feature-flag page, first-adapter target.

### 7.3 Blocked on infrastructure

- Physical hardware fault lab (BRD-STO-004 power-loss, BACE-PERF-*)
- Cross-platform CI (Windows/macOS, BRD-PORT-001)
- Multi-node network lab (BRD-CL-*, BACE-FR-NET-*)
- Independent penetration review (BRD-SEC-012)
- Published benchmark corpus (BRD-OPS-008)
- External SQL reference suite host (G1-REFERENCE)

### 7.4 Code-only follow-ups still achievable

From `BRADIAN_COMPREHENSIVE_GAPS_ANALYSIS.md` §9. All are small vertical slices with no §13 or infrastructure dependency:

- **N** — Observational `resource_usage()` aggregator across catalog + domain + MVCC.
- **O** — Fault-recovery matrix extension: lock-file deletion mid-session; WAL header with declared body but zero body bytes; parallel-open race under the existing advisory lock.
- **P** — Cite additional existing `durable_mvcc` / `history_sql` schedules in `BRADIAN_ISOLATION_LEVELS.md`.

These were queued at session end but not yet started.

## 8. Metrics

- **20 commits** on `main`, every one pushed, every one with `scripts/verify_roadmap.py` passing.
- **+83 tests** (772 → 855), 0 failures throughout the session.
- **3 new ADRs** (0056, 0057, 0058) plus one amendment (0058).
- **3 new planning / reference documents** (implementation plan, module review, gaps analysis).
- **2 new reference documents** (SQL subset, isolation levels) + 2 extensions.
- **5 new operations documents** (1 index + 4 guides).
- **17 verification reports** in `docs/implementation/`.
- **5 new admission types** and **4 new `ResourceError` variants**.
- **7 new `open_with_limits` entry points** across the engine composition.

## 9. Session-end headline statement

The crate is at the honest limit of single-process Gate 1 scope.
Every decoded-state growth vector identified during the session is
behind a named `ResourceError` admission variant. Every SRS v1.1
requirement ID from §16 + §31 is traced to a shipped module or
explicitly OPEN; the gaps analysis separates release-blocking MUST
obligations from feature shortfalls so release readiness can be
assessed honestly. BACE remains a parallel SRS with zero backing
code; its first vertical slice requires signed ADRs for the central
policy service, codec registry, BRPG v2 feature-flag page, and
compressed envelope. Beyond the three enumerated code-only
follow-ups above, further progress requires one of the twelve §13
owner decisions or one of the six infrastructure procurements.

See [`docs/implementation/BRADIAN_COMPREHENSIVE_GAPS_ANALYSIS.md`](docs/implementation/BRADIAN_COMPREHENSIVE_GAPS_ANALYSIS.md)
for the full gap decomposition and priority matrix.
