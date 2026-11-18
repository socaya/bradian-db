# ADR 0169 — HTTP /metrics endpoint (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-18.** Completes Batch J
by wiring ADR 0125's Prometheus text renderer to a
public HTTP route.

## 1. Scope

Changes in `bradian-serving`:

- `ServerState.metrics:
  bradian_relational::metrics::MetricsRegistry` — new
  shared field; `ServerState::new` instantiates it.
- `/metrics` GET route mounted ALONGSIDE `/healthz`,
  outside the auth middleware (Prometheus scrapers
  historically expect no auth).
- Handler renders `state.metrics.render_prometheus()`
  with `Content-Type: text/plain; version=0.0.4`.

New workspace dependency: `bradian-relational` in
`[workspace.dependencies]` + the serving crate's
`[dependencies]`.

## 2. Out of scope

- **Scrape authentication** — follow-up ADR can gate
  behind a static scrape-token header or
  mTLS-identity check when network identity story
  lands.
- **Metrics autowiring** — the registry is empty by
  default; application code must register counters /
  histograms to populate the endpoint.
- **OpenMetrics MIME** — still `text/plain` per
  ADR 0125 §2.

## 3. Compliance

New integration test file
`crates/bradian-serving/tests/metrics_endpoint.rs`
(4 tests):

- `metrics_endpoint_is_public_and_serves_text_plain`.
- `metrics_endpoint_renders_registered_counters`.
- `metrics_endpoint_is_reachable_without_auth_token`.
- `protected_analytics_endpoint_still_requires_auth`
  (sanity check — not everything is suddenly public).

## 4. Honest status

**Batch J (Driver + metrics endpoint)** reaches
100%:
- J1 Driver trait + adapters (ADRs 0129 + 0158 +
  0160 + prepared-statement + transport).
- J2 Prometheus renderer (ADR 0125) +
  **HTTP endpoint (this ADR)**.
