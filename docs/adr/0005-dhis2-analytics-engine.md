# ADR 0005 — DAP as a DHIS2 analytics engine

**Status:** Accepted · 2026-11-11
**Spec sections:** §1, §8, §10 · requirements DHI-001..004, API-001

## Context

DHIS2 can be configured to delegate its analytics workload to an external engine. Today
the two external engines in production use are Apache Doris and ClickHouse, wired via
`dhis.conf` properties like:

```properties
analytics.database = doris
analytics.database.jdbc.url      = jdbc:mysql://...
analytics.database.jdbc.username = ...
```

DHIS2 then invokes the engine for its analytics-table build, indicator evaluation and
reporting routes. The in-process Postgres path remains the default.

DAP is a general-purpose analytical platform (ADR 0004) that already speaks DHIS2
semantics via `dap-dhis2` and the live metadata adapter (`dap-catalog::dhis2_http`). We
want the same `dhis.conf` ergonomics so a DHIS2 operator can swap Doris / ClickHouse for
DAP without changing the DHIS2 server code they ship. This ADR pins the contract and the
on-disk / on-wire shape.

## Decision — HTTP-first, JDBC-optional

DHIS2's analytics-engine interface is JDBC-shaped today. DAP exposes an HTTP contract
(see §10 of the spec) because it is simpler, cheaper to run on edge sites, and avoids a
proprietary wire protocol. We bridge the two with:

1. **Rust-side HTTP contract** — the `dap-serving::dhis2_compat` module. Routes mirror
   the DHIS2 public analytics surface:
   - `POST /api/analytics.json` — aggregate analytics (DONE, v1)
   - `POST /api/analytics/events/query.json` — event analytics (adapter scheduled)
   - `POST /api/analytics/enrollments/query.json` — tracker analytics (adapter scheduled)
   - `POST /api/analytics/rawData.json` — deliberately rejected; DHIS2 operators should
     use `/api/analytics/v1/drilldown` with an explicit limit.
2. **DHIS2-side plug-in** — a thin Java module (ships separately under
   `apps/dhis2-plugin/` once DHIS2 version compatibility is pinned) that implements
   DHIS2's `AnalyticsDatabase` service provider. It builds query envelopes from DHIS2's
   internal AST, posts them to the DAP base URL and lifts the response.

The HTTP contract is sufficient for every DHIS2 analytics user journey (visualiser,
pivot tables, event reports, enrollment line listings) because DHIS2 never pushes raw
SQL to the engine — it submits a structured request object.

## `dhis.conf` properties

```properties
# DHIS2 version 2.41 onwards.
analytics.database               = dap

# DAP base URL. Must speak HTTPS in production. Trailing slash optional.
analytics.database.base_url      = https://dap.internal:8443

# Authentication: basic | bearer | none.
analytics.database.auth          = basic
analytics.database.auth.username = dhis2-reader
analytics.database.auth.password = ${DAP_READER_PASSWORD}

# Optional: override timeouts (ms). Defaults: connect=5000, read=60000.
analytics.database.connect_timeout_ms = 5000
analytics.database.read_timeout_ms    = 60000

# Optional: pin a partition id prefix. DAP partitions are logical, so the plug-in
# derives `{dataset}/{partition_prefix}-{yearly | monthly | all}` by default.
analytics.database.partition_prefix = p

# Optional: provenance mode surfaced in DHIS2 responses. One of:
#   strict  — fail the request if DAP returns `is_exact=false`.
#   warn    — surface a warning header but return the rows.
#   lenient — ignore provenance stamps (not recommended).
analytics.database.provenance = warn
```

## Response envelope

DAP's `dhis2_compat` layer emits the DHIS2 analytics envelope so the DHIS2 server can
parse it without special-casing:

```json
{
  "headers": [
    { "name": "ou", "column": "ou", "valueType": "TEXT", "type": "java.lang.String",
      "hidden": false, "meta": false },
    { "name": "period", "column": "period", "valueType": "TEXT", "type": "java.lang.String",
      "hidden": false, "meta": false },
    { "name": "sum_value", "column": "sum_value", "valueType": "NUMBER", "type": "java.lang.String",
      "hidden": false, "meta": false }
  ],
  "rows": [
    ["ouAAAAAAAAA", "202601", 42.0]
  ],
  "metaData": { "items": {}, "dimensions": {} },
  "width": 3,
  "height": 1,
  "rowContext": {},
  "provenance": {
    "data_revision": 3,
    "metadata_revision": "…",
    "watermark_ms": 1700000000000,
    "is_exact": true,
    "engine": "dap"
  }
}
```

The DHIS2 shape is preserved; the extra `provenance` block is DAP-specific and lets
DHIS2 clients decide whether to surface a "stale" or "partial" badge in the UI.

## Guarantees DAP keeps for DHIS2

1. **Correct aggregation-type roll-ups** (DHI-001). Every value the engine emits applied
   the right time + OU reduction for that data element's `aggregationType` (`aggregate`
   module).
2. **Period semantics** (DHIS2 periods). Weekly variants, bi-monthly, six-monthly,
   financial years all resolve the same way the DHIS2 reference implementation resolves
   them (`dap_core::period`).
3. **Reporting rates** (`#{dsUid.REPORTING_RATE}` and friends). Resolved through the
   `completion` fact store; zero expected ⇒ `NULL`.
4. **Fail-closed on unsupported constructs**. Unknown `d2:` functions, nested CUSTOM
   aggregators, or references to data elements the catalog does not know reject with a
   precise diagnostic. Never a silent approximation.
5. **Provenance on every response**. Data revision, metadata revision, watermark, site
   coverage, exact / approximate flag. DHIS2 operators can audit staleness directly.

## Routes DAP intentionally does not implement

- **`/api/analytics/rawData.json`** — unbounded line listings do not fit DAP's bounded
  export contract. Use `/api/analytics/v1/drilldown` with an explicit limit.
- **Arbitrary SQL endpoints** — spec §11 explicitly forbids them. DHIS2 never asks for
  arbitrary SQL from an external engine so this is not a regression.

## Migration from Doris / ClickHouse

1. Freeze DHIS2's analytics-table build while DAP ingests the existing DataValue table
   via the DAP SQL-CDC adapter (`dap-sources::sql_cdc`).
2. Point DHIS2's `dhis.conf` at DAP by setting `analytics.database = dap`.
3. Compare parity for N days with DAP in warn mode and DHIS2's existing engine in strict
   mode. The `dhis2-parity` matrix in the UI surfaces diffs per analytical family.
4. Flip the primary engine when parity has held.

Rollback is a one-line change in `dhis.conf`.
