# ADR 0004 — DAP as a general-purpose analytical platform

**Status:** Accepted · 2026-11-11
**Spec sections:** §1, §2.2, §10

## Context

DAP ships alongside DHIS2-specific metadata and indicator semantics (`dap-dhis2`,
`dap-catalog::dhis2_adapter`), but the core engine is not DHIS2-dependent. Any data system
that can describe its facts as `(key, attributes, value)` rows can plug in. We want that
boundary explicit so operators and future adapters have a single place to look.

## Decision — three-layer architecture

```
                        serving / distributed / security
                                        │
                              planner / runtime / views
                                        │
                              storage / core
                                        │
                     sources (generic)          domain adapters
                     ├─ csv                     ├─ dhis2
                     ├─ jsonl                   │   ├─ expression + indicator compiler
                     ├─ sql_cdc                 │   ├─ aggregate / event / tracker
                     └─ (arrow ipc, parquet,    │   ├─ reporting-rate + relationships
                         grpc streams …)        │   └─ metadata adapter
                                                ├─ (openmrs, hapi fhir, odoo, …)
                                                └─ (your domain here)
```

Layers top-down:

1. **Serving / distributed / security** — HTTP/JSON API, federation coordinator,
   authentication + authorisation + encryption. None of these name DHIS2.
2. **Planner + runtime + views** — logical / physical plan, vectorized operators,
   incremental materialisation. Plan shapes are domain-neutral (`AggregateQuery`,
   `ProgramIndicatorQuery`). `ProgramIndicatorQuery` is DHIS2-flavoured in name but is
   just an indirection through a catalog object — any adapter that registers an
   "indicator" in the catalog can use the same code path.
3. **Storage + core** — `.derc` column segments, WAL, manifests, delta, snapshots, typed
   `Value` / `Schema` / `Period` primitives. Zero domain knowledge.

The two adapter wings below `storage/core` are what make the platform ship-ready for a
given deployment.

## Generic sources (`dap-sources`)

Per-format readers that produce canonical `dap_ingest::Change` envelopes. v1 modules:

- `csv` — RFC 4180-ish parser with escape rules; `read_csv_bytes` + `rows_to_changes`.
- `jsonl` — `read_jsonl` for pre-shaped envelopes, `read_jsonl_with` for projection.
- `sql_cdc` — trait `SqlCdcSource` + `rows_to_changes` for Debezium / wal2json / binlog
  rows.

Add new formats by writing another module. The audit surface stays small — v1 does not
pull in a CSV or Arrow crate.

## Domain adapters

A domain adapter answers two questions:

1. **Metadata** — what does a "dataset", "program", "indicator" look like in this domain,
   and how do I load them into the generic [`Catalog`]? `dap-catalog::dhis2_adapter`
   demonstrates the pattern for DHIS2.
2. **Analytical semantics** — any domain-specific expression grammar, aggregation rules,
   or fact models live in a dedicated crate. `dap-dhis2` holds the full DHIS2 analytical
   surface (expressions, aggregate / event / tracker programs, indicators, custom
   aggregation). An OpenMRS or FHIR adapter would be a sibling crate at this level.

The core engine never imports a domain adapter. Dependency direction is strictly:
domain → core. If you ever find core importing a domain module, that is the signal to
refactor.

## Deployment profile matrix

| Deployment | Needs | Does not need |
|---|---|---|
| DHIS2 ministry stack | `dap-dhis2`, catalog DHIS2 adapter | nothing else |
| HMIS data lake with CDC | `dap-sources::sql_cdc`, domain-neutral queries | DHIS2 crate |
| Research cohort study | `dap-sources::csv` + a cohort-specific metadata loader | DHIS2 crate |
| FHIR analytics | new `dap-fhir` crate + optional JSONL source | DHIS2 crate |
| Offline edge site (e.g. refugee camp registry) | core engine + any source adapter | serving cluster, distributed federation |

Every deployment uses the same `.derc` format, the same vectorized runtime, the same
query planner. Only the wings on either side of the core differ.

## Consequences

- `dap-dhis2` is a leaf crate — nothing in the core, planner, runtime, or serving layers
  depends on it. We enforce this via `cargo test --workspace` on CI; a build regression
  there would surface immediately.
- `dap-sources` grows one module per format. New modules only depend on
  `dap-core`/`dap-ingest`/`dap-storage`; they never pull in a domain crate.
- The HTTP API carries DHIS2-flavoured query names (`ProgramIndicatorQuery`) but the
  payloads are plain UIDs + expression strings; a non-DHIS2 adapter can register its own
  catalog entries under the same shape without changing the API surface.
