# DHIS2 upstream integration — DAP as an analytics backend

This directory is **not a runtime plugin**. It carries the source files DHIS2 upstream
needs to include so that a future DHIS2 release ships with DAP as a first-class analytics
backend — exactly the model DHIS2 **v43** used to add Apache Doris.

## How DHIS2 v43 added Doris (the model we mirror)

DHIS2 v43 did not load Doris as a JAR plugin. It added Doris support directly to the
DHIS2 source tree:

| Area | DHIS2 v43 change |
|---|---|
| `dhis-api` enum | `AnalyticsDatabase.DORIS` added to `org.hisp.dhis.analytics.AnalyticsDatabase` |
| Query manager | `DorisAnalyticsManager implements AnalyticsManager` under `dhis-services-analytics` |
| Table manager | `DorisAnalyticsTableManager implements AnalyticsTableManager` for the analytics-table rebuild |
| Config provider | Reads `analytics.database = doris` + `analytics.database.jdbc.url` + credentials from `dhis.conf` |
| Driver wiring | MySQL JDBC driver on the classpath (Doris speaks MySQL wire-protocol) |
| Docs | `dhis.conf` reference + ops runbook |

Operators then just set `analytics.database = doris` in their existing DHIS2 install —
no extra JARs, no classpath surgery, no service-loader ceremony.

## The same shape for DAP

Everything in this directory is sized to drop straight into the DHIS2 source tree as a
upstream patch / PR. The package names (`org.hisp.dhis.analytics.dap.*`) match the
convention DHIS2 uses for its other analytics backends (`org.hisp.dhis.analytics.doris`,
`org.hisp.dhis.analytics.postgres`).

### What this directory ships

```
src/main/java/org/hisp/dhis/analytics/dap/
├── DapAnalyticsBackend.java            enum constant + name() that AnalyticsDatabase.DAP maps to
├── DapConfiguration.java               typed read of `analytics.database.*` properties from DHIS2's DhisConfigurationProvider
├── DapHttpClient.java                  one java.net.http.HttpClient per DHIS2 instance
├── DapAnalyticsManager.java            implements AnalyticsManager for query execution
├── DapAnalyticsTableManager.java       implements AnalyticsTableManager for analytics-table build
├── DapRequestEnvelope.java             serialisable request posted to DAP
├── DapResponseEnvelope.java            serialisable response parsed back
└── DapIngestBatch.java                 batch of canonical changes sent by the table-manager during rebuild
```

The `AnalyticsManager` / `AnalyticsTableManager` interfaces named above are DHIS2's own
— they live under `org.hisp.dhis.analytics` in `dhis-api` and `dhis-services-analytics`.
To keep this directory buildable without a full DHIS2 checkout, we declare a tiny
version of each interface alongside our implementation. The real DHIS2 build swaps our
copies for the authoritative ones at compile time.

### DAP side

DAP exposes the HTTP endpoints DHIS2 posts to during the analytics-table build:

- `POST /api/analytics/v1/ingest` — accept a batch of canonical `Change` envelopes
  (DHIS2's analytics-table rebuild path lands here instead of running SQL DDL).
- `POST /api/analytics.json` and friends — the DHIS2-shape analytics query endpoints
  (`dap-serving::dhis2_compat`, documented in ADR 0005).

The ingest endpoint is new in this cycle — see the Rust side under
`crates/dap-serving/src/dhis2_compat.rs` and the integration test in
`crates/dap-serving/tests/dhis2_compat.rs`.

### Operator configuration

Once the DHIS2 release including these sources is deployed, operators set:

```properties
analytics.database               = dap
analytics.database.base_url      = https://dap.internal:8443
analytics.database.auth          = basic
analytics.database.auth.username = dhis2
analytics.database.auth.password = ${DAP_PASSWORD}

# Analytics table rebuild knobs.
analytics.database.ingest.batch_rows      = 10000
analytics.database.ingest.max_concurrency = 4

# Provenance: strict / warn / lenient — mirrors DHIS2's handling for Doris.
analytics.database.provenance = warn
```

No `jar` to install, no service-loader descriptor, no extra classpath entries. The
DHIS2 release ships with DAP support in the same way v43 ships with Doris support.

## PR plan (how to land this upstream)

1. Open a DHIS2 issue requesting a new `AnalyticsDatabase` entry for DAP, referencing
   ADR 0005 (`docs/adr/0005-dhis2-analytics-engine.md`) and this directory.
2. Rebase these source files into the DHIS2 fork under:
   - `dhis-2/dhis-api/src/main/java/org/hisp/dhis/analytics/AnalyticsDatabase.java`
     (add the enum constant)
   - `dhis-2/dhis-services/dhis-service-analytics/src/main/java/org/hisp/dhis/analytics/dap/`
     (every other file)
3. Match DHIS2's existing Doris wiring for the service-registration glue:
   `AnalyticsManagerFactory`, `AnalyticsTableManagerFactory`, `DhisConfigurationProvider`.
4. Add a DHIS2 parity test that runs a reference aggregate query against DAP and
   compares the response to the DHIS2 built-in PostgreSQL engine's output.
5. Submit the PR with ADR 0005 attached.

Until the PR lands, operators can apply these files as a DHIS2 source patch.

## Tests

The Java tests in this directory exercise the HTTP client + configuration parser
without a running DHIS2. They give us a fast feedback loop on wire-shape regressions.
The DHIS2-side integration test (step 4 above) lives inside the DHIS2 PR because it
needs the full DHIS2 build.
