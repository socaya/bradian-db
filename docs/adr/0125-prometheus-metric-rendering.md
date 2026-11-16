# ADR 0125 — Prometheus text-format rendering for MetricsRegistry (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch J J2 primitive.
Batch J J1 (local driver facade) requires deeper design
work and is deferred to a follow-up ADR.

## 1. Scope

- `MetricsRegistry::render_prometheus() -> String` emits
  the counter and histogram snapshot as the Prometheus
  text exposition format (v0.0.4).
- Counters emit `# TYPE <name> counter` + a single
  `<name> <value>` line.
- Histograms emit `# TYPE <name> histogram` plus one
  `<name>_bucket{le="<bound>"}` per cumulative bucket, a
  final `<name>_bucket{le="+Inf"}`, `<name>_sum` and
  `<name>_count`.
- `sanitize_metric_name` replaces any byte outside the
  Prometheus-legal `[a-zA-Z_:][a-zA-Z0-9_:]*` with `_` so
  the Bradian `ops.total` dotted convention renders as
  `ops_total`.
- Deterministic ordering (BTreeMap iteration) so output
  is stable for diffing.

## 2. Out of scope (next ADRs)

- **HTTP `/metrics` endpoint wiring** — the renderer
  returns a String; mounting it on an axum route under
  `bradian-admin` (or a new `bradian-serving` crate)
  needs authn, rate-limiting and endpoint-specific tests.
- **Labels** — the current counter / histogram keys are
  name-only. Real Prometheus labels (`{job="bradian",
  tenant="..."}`) would require a labeled-metric
  primitive.
- **OpenMetrics** — the newer `application/openmetrics-
  text` MIME + `# UNIT` / `# HELP` / `# EOF` sentinels
  are not emitted. Operators can rewrite downstream.
- **Exemplars** — the current histogram is bucket-only.
- **Gauges / Summaries** — only Counter + Histogram
  primitives exist. Gauge semantics need a dedicated
  primitive.

## 3. Compliance

- Counter emits `# TYPE … counter` + value line.
- Histogram emits cumulative buckets (strictly monotone)
  + `+Inf` bucket + `_sum` + `_count`.
- Non-alphanumeric characters sanitized to `_`.
- Empty registry returns empty string.
- Alphabetical name order (BTreeMap).
- 5 new tests in `metrics::tests`.

## 4. Honest status

Foundation only. **BRD-OBS-002 Prometheus-compatible
telemetry endpoint** is not yet satisfied because there
is no HTTP endpoint and no labeled-metrics primitive.
The renderer is a prerequisite that a future ADR can
mount on a secured route.
