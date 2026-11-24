# ADR 0306 — DHIS2 synthetic fact generator: workload presets

## Status

Accepted. First slice of P1-5 (benchmarks).

## Context

`bradian_benchmark::fixtures::generate_facts` already
produces a reproducible set of DHIS2 aggregate rows for a
given (seed, org_units, periods, data_elements, rows)
tuple. P1-5 asks for two additional properties:

- **Reproducibility** — proven, not merely implied by the
  deterministic RNG seed.
- **Documented workload** — named presets so bench and
  comparison runs reference the same shapes.

## Decision

Additive, inside `bradian_benchmark::fixtures`:

1. `WorkloadSpec { seed, org_units, periods, data_elements, rows }` is
   the typed description of a workload, with three named
   presets:
   - `WorkloadSpec::small()`      — 50 OUs × 24 periods ×
     200 DEs × 100 000 rows (seed `1`).
   - `WorkloadSpec::medium()`     — 500 × 36 × 1 000 ×
     2 000 000 (seed `2`).
   - `WorkloadSpec::dhis2_like()` — 5 000 × 24 × 2 000 ×
     10 000 000 (seed `7`).
2. `generate_from_spec(spec: &WorkloadSpec) -> FactsFixture`
   convenience wrapper.
3. A unit test `determinism_across_two_calls` asserts the
   `small()` preset returns byte-identical rows on two
   consecutive calls.
4. A unit test `presets_are_distinct_fingerprints` asserts
   the three presets' row counts match the documented
   values.

Scope limits:

- The presets live in Rust; they are not persisted as
  YAML / JSON. If an external harness needs the same
  shapes it can call into this crate or re-declare them.
- No wall-time benchmark is run in this slice — publishing
  Criterion numbers belongs to ADR 0307.

## Consequences

- Benches can refer to `WorkloadSpec::small()` et al.
  instead of duplicating magic numbers.
- Determinism is covered by an explicit test, not a
  comment.
- 2 new unit tests.
