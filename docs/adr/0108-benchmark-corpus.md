# ADR 0108 — Published benchmark corpus (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.**

## 1. Scope

- Extends ADR 0094's synthetic DHIS2-like harness with a
  sweep across `max_rows_per_page ∈ {8, 64, 512, 4096}` ×
  `use_column_dict ∈ {off, on}`.
- New bench:
  `crates/bradian-benchmark/benches/bace_sweep.rs`.
- Published results doc:
  `docs/implementation/BRADIAN_BACE_SWEEP_RESULTS_2026_11_16.md`
  with the full 8-row size + write + read table.

## 2. Operator guidance extracted

| Choice | When |
|---|---|
| `max_rows_per_page=512, column_dict=off` | **Default.** Within 25% of minimum size + fastest read on mixed workloads. |
| small page + `column_dict=on` | Dict-heavy low-cardinality columns (~33% win at page=8). |
| large page + any `column_dict` | Plain ZSTD self-learns; column-dict near-zero improvement (~1%). |

## 3. Out of scope

- Real DHIS2 production data (owner procurement).
- Multi-threaded reader benchmarks.
- Network-filesystem cold-read measurements.
- Cross-platform throughput (run only on Linux).

## 4. Compliance

1. Bench compiles + runs via
   `cargo bench -p bradian-benchmark --bench bace_sweep`.
2. 8 variants round-trip correctly.
3. Observed numbers recorded with ADR 0094's "not universally
   smaller" honest callout cross-validated.
4. Results doc explicit about synthetic / Linux-only / single-
   threaded boundaries.
