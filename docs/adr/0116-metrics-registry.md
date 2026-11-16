# ADR 0116 — In-process metrics registry (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch F F2.

## Scope

- `bradian_relational::metrics::MetricsRegistry` with shared
  state via `Arc<RwLock>`.
- `Counter` (atomic u64, increment/add/get).
- `Histogram` with operator-chosen bucket upper-bounds +
  atomic bucket counts + sum + count.
- `HistogramSnapshot::mean()` helper.
- Get-or-create semantics by name so multiple call sites
  share one counter without coordination.

## Out of scope

- HTTP / Prometheus / OpenTelemetry endpoints — those live
  in `bradian-serving`.
- Automatic instrumentation of engine paths — engines opt
  in by holding a registry handle and calling `.counter()`.
- Histogram percentile estimators — overflow bucket only.

## Compliance

- Counter atomic increment/add/get.
- Shared counters across multiple get-by-name calls.
- Histogram buckets count correctly + overflow bucket.
- Empty histogram `mean()` returns None.
- Snapshot sorts by name (BTreeMap).
- 5 unit tests.
