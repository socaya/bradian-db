# Bradian Database Systems — SRS v1.1 implementation direction

**Authoritative target**: `docs/Bradian_Master_Architecture_SRS_v1.1.md`.
**Source audit**: `docs/implementation/BRADIAN_V1_1_AUDIT.md`.
**Previous foundation**: `docs/DER_Analytics_Platform_Architecture_and_Feature_Specification.md`.

This repository started as the DAP analytical engine. Preserve working analytical code, `dap-*` crate imports, existing DHIS2 adapters and persisted `.derc` files until a tested compatibility migration exists. Bradian's full relational OLTP/HTAP/cluster/security targets are **not implemented** by renaming the product.

Security: only the local temporary administrator bearer-token bridge has been introduced. All full SQL, durable authorization and TLS/mTLS work remains release-blocking. No unauthenticated API exposure or unsafe non-loopback bind is permitted in the bootstrap profile.

Before every implementation milestone: audit code and v1.1 requirements, write an ADR for any durable format/protocol, add correctness/authorization/crash tests, run workspace CI, record evidence in `docs/gates/`, and avoid claiming partial functionality as done.

---

# CLAUDE.md — DAP (DER Analytics Platform) Implementation Guide

This file governs implementation work on DAP. The authoritative functional/architectural source is
`docs/DER_Analytics_Platform_Architecture_and_Feature_Specification.md` (spec DER-ANL-PLAT-001 v1.0).
This file captures project-specific engineering decisions, constraints, and procedures.

## 1. Project identity

- **Platform:** DER Analytics Platform (DAP) — native, offline-capable, DHIS2-aware analytical engine.
- **Scope:** native columnar storage (`.derc`), WAL + manifests, vectorized executor, cost-based
  planner, incremental materialized views, DHIS2 semantic compiler, edge/server reconciliation,
  distributed execution, secure serving APIs, admin tooling.
- **Non-negotiable (spec §1):** NO runtime dependency on ClickHouse, Apache Doris, DuckDB, or any
  externally operated OLAP database. Low-level libraries (compression, hashing, crypto, Arrow
  interchange) are acceptable with justification. DER owns the storage format, catalog, planner,
  execution orchestration and distributed coordination.

## 2. Language and toolchain

- **Primary language:** Rust (edition 2024). Chosen for: zero-cost abstractions over columnar
  batches, portable SIMD, predictable memory ownership for storage engines, strong fuzzing /
  property-based tooling, small edge binaries, excellent async runtime for serving.
- **Minimum Rust:** stable 1.85+ (uses 2024 edition).
- **Build system:** Cargo workspace with one crate per logical module (§2.2 of the spec).
- **No unsafe** outside clearly-scoped `storage::codec` + SIMD kernels; every `unsafe` block carries
  a `// SAFETY:` comment explaining the invariant.

## 3. Workspace layout

```
dap/
  Cargo.toml                 # workspace manifest
  rust-toolchain.toml
  crates/
    dap-core/                # typed value model, UIDs, errors, time/period, config
    dap-catalog/             # versioned DHIS2 semantic catalog + lineage
    dap-storage/             # .derc format, WAL, manifests, compaction, delta
    dap-runtime/             # column batches, vectorized operators, scheduler
    dap-planner/             # parser, binder, logical plan, CBO, EXPLAIN
    dap-ingest/              # canonical change log, idempotent ingestion
    dap-views/               # incremental materialized views, retractions
    dap-dhis2/               # DHIS2 aggregate/event/tracker/indicator semantics
    dap-distributed/         # coordinator, workers, exchange, reconciliation
    dap-serving/             # HTTP/JSON API (axum), OpenAPI schemas
    dap-security/            # authn/authz, row/column/OU policy, audit
    dap-admin/               # operator CLI + diagnostics endpoints
    dap-benchmark/           # reproducible benchmark harness
  tests/                     # cross-crate integration, chaos, e2e
  benches/                   # criterion harnesses referenced by dap-benchmark
  docs/                      # spec + ADRs
  scripts/                   # dev helpers
  fixtures/                  # synthetic + anonymised test corpora
```

Layout mirrors the logical modules in spec §2.2 but is not sacrosanct — refactor when boundaries
no longer carry weight.

## 4. Engineering standards

1. **No scaffolding-as-feature.** Do not mark a requirement complete without working code AND tests.
   Partial implementations must be labeled `#[cfg(todo)]` or explicitly listed in the gate exit
   report.
2. **Correctness oracle first.** For every vectorized operator, land a scalar reference
   implementation. Property tests must assert `vector == scalar` on random inputs.
3. **Deterministic by default.** Thread parallelism is opt-in via `ExecContext`; tests fix seeds.
4. **Metadata-driven.** Zero hardcoded program IDs, dataset IDs, period types, or indicator names.
5. **Fail closed.** Unknown segment feature flags → refuse to open. Unknown DHIS2 function →
   reject at compile time with precise diagnostic. Never silently approximate.
6. **Durability before performance.** Ship atomic manifest + crash recovery before tuning codec
   cost models.
7. **Consistency metadata is part of every result.** Every `QueryResult` carries
   `{data_revision, metadata_revision, consistency_mode, watermark, site_coverage, is_exact}`.
8. **Snapshot-safe reads.** Readers hold a snapshot handle; compaction/GC cannot delete pages
   referenced by live snapshots.
9. **Spec-first binary formats.** `.derc` and WAL layouts are specified in
   `docs/adr/` with byte diagrams before implementation.
10. **Benchmark hygiene.** No perf number quoted without: hardware, dataset size, concurrency,
    cache state, code revision, raw criterion output archived under `benches/results/`.

## 5. Dependency policy

Allowed library categories (keep the set small, pin versions, review licences):

| Purpose | Preferred crate | Rationale |
|---|---|---|
| Serialisation | `serde`, `serde_json`, `bincode` | Standard, audited |
| Error handling | `thiserror`, `anyhow` (bins only) | Idiomatic |
| Logging/tracing | `tracing`, `tracing-subscriber` | Structured observability |
| Async runtime | `tokio` (multi-thread) | Required for serving |
| HTTP | `axum`, `tower`, `tower-http` | Minimal, typed |
| Compression | `zstd`, `lz4_flex` | Audited, pure-Rust available |
| Hashing | `ahash`, `xxhash-rust`, `blake3` | Non-cryptographic + cryptographic |
| Checksums | `crc32fast` | WAL + page checksums |
| Columnar interchange (optional adapter) | `arrow` | Only in the explicit adapter; never in hot paths |
| Testing | `proptest`, `insta`, `criterion`, `tempfile` | Property + snapshot + bench |
| SIMD | `std::simd` (nightly-portable) when stable, otherwise feature-detected | Portable fallback mandatory |

Anything outside this list requires an ADR entry justifying it.

## 6. Git & commit workflow

- Author/committer: `socaya <stephocay@gmail.com>` (already configured).
- Conventional commits: `feat(scope): …`, `fix(scope): …`, `refactor(scope): …`,
  `test(scope): …`, `docs(scope): …`, `chore(scope): …`, `bench(scope): …`.
- Scope matches crate name without the `dap-` prefix (`core`, `storage`, `runtime`, …) or `workspace`.
- Commit per completed, verified milestone. Each commit must build (`cargo check --workspace`)
  and pass its own tests (`cargo test -p <crate>`).
- **No** `Co-Authored-By`, no `Generated with Claude Code`, no AI attribution trailers.
- Push to `origin/main` when authentication permits; never force-push.

## 7. Verification gates (map to spec §15)

| Gate | Focus | Required exit artefacts |
|---|---|---|
| G0 | Audit + traceability | `docs/traceability.md`, `docs/adr/0001-language-and-workspace.md` |
| G1 | Types, catalog, scalar oracle, query DSL stub | `dap-core`, `dap-catalog`, end-to-end round trip over fixture data |
| G2 | `.derc`, WAL, manifests, delta, compaction | Format spec ADR + crash/corruption tests |
| G3 | Vectorized scan/filter/aggregate/sort/spill | Scalar-oracle equivalence tests + baseline criterion bench |
| G4 | Ingest journal + materialized views + retractions | Offline-correction correctness fixture |
| G5 | Indexes, stats, CBO, joins, parallelism | Measured speedup, no accuracy regression |
| G6 | DHIS2 aggregate/event/tracker/indicator semantics | Parity fixture matrix |
| G7 | Distributed/federated execution + reconciliation | Multi-node correctness + outage simulation |
| G8 | Serving API + data products + admin | E2E operator journeys |
| G9 | Production qualification | Benchmark report, recovery drill, rollback runbook |

Each gate's exit evidence goes in `docs/gates/G<N>.md` (not written until the gate's work lands).

## 8. Testing strategy (spec §17)

- `cargo test --workspace` must be green on every commit.
- Property tests (`proptest`) live beside the unit they cover.
- Integration tests live in `crates/<name>/tests/` or the top-level `tests/` for cross-crate flows.
- Fuzz targets live in `fuzz/` (cargo-fuzz) — added once storage reader stabilises.
- Benchmarks live in `benches/` and are reproducible via `scripts/bench.sh`.
- DHIS2 parity fixtures live in `fixtures/dhis2/` and are regenerated by documented scripts.

## 9. Security guardrails (spec §11)

- No arbitrary filesystem, process, or network access reachable from the DSL / SQL surface.
- Policy is evaluated in the binder; the executor never receives unfiltered dimension bindings.
- Cache keys include the authorisation context hash; no shared cache line between users with
  distinct OU/field permissions.
- All serving endpoints enforce auth; unauthenticated requests reach only `/healthz`.

## 10. Execution procedure for this agent

When continuing this project:

1. Re-read this file + the spec summary in `docs/traceability.md`.
2. Inspect the last 5 commits to understand the current state.
3. Pick the next unresolved item from the traceability matrix.
4. Implement, test (`cargo test -p <crate>`), lint (`cargo clippy --all-targets -- -D warnings`
   when configured), and commit with a conventional message.
5. Update `docs/traceability.md` to reflect the new status.
6. Push if upstream is reachable.

Do not create new top-level docs unless a user asks or a spec clause requires it. Place ADRs
under `docs/adr/`. Place gate evidence under `docs/gates/`.
