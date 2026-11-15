# ADR 0094 — Synthetic DHIS2-like fact-table BACE benchmark (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-15 and implemented.** See the [verification doc](../implementation/BRADIAN_BACE_DHIS2_BENCHMARK_VERIFICATION_2026_11_15.md) for the honest observation that v1.4 column-dict is **2.4× LARGER** than v1.0 on this workload — column-dict's sweet spot is large dicts × many pages, not small-dict × small-page.

Honest scope note: real DHIS2 workload data is not in this
repo. This ADR delivers a **synthetic** fact-table benchmark
using the existing `FactsFixture` generator — realistic schema
and cardinalities but not real-world data. It compares `.brcs`
write size and read throughput across five variants to
quantify the BACE compression story.

## 1. Scope

**In scope:**
- New criterion bench `dhis2_like` under
  `crates/bradian-benchmark/benches/`.
- Five variants compared:
  - v1.0 — no BACE (legacy writer)
  - v1.1 — BACE wrapping (ZSTD codec)
  - v1.2 — BACE + per-column dict pool (ADR 0079)
  - v1.4 — column-level shared dict (ADR 0083)
  - v1.4 + BACA — authenticated (ADR 0080/0090)
- Workload: 200 OUs × 24 periods × 50 DEs × 3 COCs ≈ 720k
  rows, generated deterministically via seed 42.
- Output: Criterion bench report (HTML + stdout table) +
  markdown summary in verification doc.

**Out of scope:**
- Real DHIS2 production data.
- Comparison against Parquet / other analytical formats.
- Write-latency measurement at per-row granularity.

## 2. Compliance

1. Bench compiles and runs via `cargo bench -p
   bradian-benchmark --bench dhis2_like`.
2. All five variants round-trip correctly.
3. Observed sizes recorded in the verification doc.
4. 1137 pre-ADR-0094 workspace tests pass.
5. Clippy clean.
