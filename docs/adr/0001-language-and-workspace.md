# ADR 0001 — Language, toolchain and workspace topology

**Status:** Accepted · 2026-11-11
**Spec sections:** §1, §2.2, §15

## Context

The DER Analytics Platform specification mandates native columnar storage, vectorized execution,
WAL + atomic manifests, and an offline-first edge profile. The engine must not depend on
ClickHouse, Doris, DuckDB, or any externally operated OLAP runtime. Edge binaries must stay small
enough to co-locate with Capture on modest facility hardware, while the server profile must
support multi-worker distributed execution.

## Decision

1. **Language: Rust** (edition 2021, MSRV 1.85). Reasons:
   - Zero-cost abstractions over typed column batches without a GC pause pattern.
   - Predictable ownership for storage engines (WAL, manifests, snapshot handles).
   - Portable SIMD (`std::simd` when stabilised; feature-detected intrinsics meanwhile) with a
     mandatory portable fallback for correctness.
   - Strong property-based + fuzz tooling (`proptest`, `cargo-fuzz`) to meet spec §17.
   - Tokio async runtime for serving and distributed coordination without over-threading.
   - Small statically-linked binaries suit the DER Client edge profile.
2. **Workspace: Cargo** with one crate per logical module from spec §2.2. Crates are logical
   boundaries; files may move between crates if boundaries no longer carry weight.
3. **JVM/DHIS2 integration** happens at the protocol boundary (HTTP/JSON over the serving API),
   not by embedding the engine inside the JVM. This keeps the engine language choice independent
   of DHIS2 server internals.

## Alternatives considered

- **Java/Kotlin** (same stack as DHIS2 core). Rejected: JIT warm-up cost conflicts with site
  freshness targets; off-heap memory management for columnar batches adds complexity; harder to
  ship a small edge binary.
- **C++** (ClickHouse / Doris precedent). Rejected: team ergonomics, higher UB surface, weaker
  modern tooling story vs. Rust.
- **Go**. Rejected: GC pauses interact badly with large hash aggregates; weaker generics for
  typed column kernels; no stable SIMD story.

## Consequences

- All server/edge components ship as a single Rust workspace; a thin JVM client (not in this
  repo) can call the HTTP API for DHIS2 app integration.
- Developers need the Rust toolchain pinned by `rust-toolchain.toml`.
- Interop with the Arrow ecosystem is available but opt-in: only in an explicit interchange
  adapter crate, not in the hot execution path.
