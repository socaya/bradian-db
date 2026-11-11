# DAP — DER Analytics Platform

Native, offline-capable, DHIS2-aware analytical engine. Columnar storage, vectorized execution,
incremental materialized views, cost-based planning, secure federated analytics.

**Specification:** [`docs/DER_Analytics_Platform_Architecture_and_Feature_Specification.md`](docs/DER_Analytics_Platform_Architecture_and_Feature_Specification.md)
**Implementation rules:** [`CLAUDE.md`](CLAUDE.md)
**Traceability matrix:** [`docs/traceability.md`](docs/traceability.md)
**ADRs:** [`docs/adr/`](docs/adr/)

## Workspace

```
crates/
  dap-core       typed value model, UIDs, periods, errors
  dap-catalog    versioned DHIS2 semantic catalog
  dap-storage    .derc segment format, WAL, manifests, compaction
  dap-runtime    column batches, vectorized operators, scheduler
  dap-planner    parser, binder, logical plan, cost-based optimizer
  dap-ingest     canonical change log, idempotent ingestion
  dap-views      incremental materialized views, retractions
  dap-dhis2      DHIS2 aggregate/event/tracker/indicator semantics
  dap-security   authentication, authorization, audit
  dap-serving    HTTP/JSON analytical API (axum)
  dap-admin      operator CLI + diagnostics
  dap-benchmark  reproducible benchmark harness
```

## Build and test

```bash
cargo check --workspace
cargo test  --workspace
```

## Status

Pre-release. See [`docs/traceability.md`](docs/traceability.md) for per-requirement status.
No runtime dependency on ClickHouse, Apache Doris, DuckDB, or any externally-operated OLAP engine
(spec §1).
