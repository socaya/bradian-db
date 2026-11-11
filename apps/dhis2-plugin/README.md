# dap-dhis2-plugin

Service-provider implementation of DHIS2's analytics-database interface that routes
DHIS2 analytics requests to a DAP HTTP engine.

**Compatibility:** DHIS2 **2.41 and later**. 2.41 is the first DHIS2 release that
publishes a pluggable analytics-engine SPI (`org.hisp.dhis.analytics.AnalyticsDatabase`).
Earlier DHIS2 versions ship only the in-process PostgreSQL analytics path and cannot
load this plug-in. Java 17 minimum, matching DHIS2 2.41's JDK requirement.

Operators configure it in `dhis.conf`:

```properties
analytics.database               = dap
analytics.database.base_url      = https://dap.internal:8443
analytics.database.auth          = basic
analytics.database.auth.username = dhis2-reader
analytics.database.auth.password = ${DAP_READER_PASSWORD}
analytics.database.provenance    = warn
```

Drop the built `jar` onto the DHIS2 classpath; DHIS2's ServiceLoader picks it up via the
`META-INF/services/org.hisp.dhis.analytics.AnalyticsDatabase` descriptor.

## Why Java

DHIS2's engine registration is Java-native (JDBC-shaped on the Doris / ClickHouse path,
service-loader-shaped on 2.41+). DAP exposes an HTTP contract. The plug-in bridges the
two without changing DHIS2 server code — DHIS2 loads this `jar`, finds our
`AnalyticsDatabaseProvider` via `META-INF/services`, and matches it to
`analytics.database = dap` from `dhis.conf`.

## Build

```bash
cd apps/dhis2-plugin
mvn -q verify
```

Runs the unit tests and the MockServer-backed integration test that asserts the plugin
posts the canonical DAP request envelope and parses the DAP response envelope correctly.

## Install into DHIS2

1. `mvn package` → `target/dap-dhis2-plugin-0.1.0.jar`.
2. Copy the jar under `${DHIS2_HOME}/plugins/` (or the classpath location your DHIS2
   deployment exposes for third-party engines).
3. Set `analytics.database = dap` plus the other properties from
   `docs/dhis.conf.example` in `${DHIS2_HOME}/dhis.conf`.
4. Restart DHIS2. The startup log must print one line matching
   `Registered analytics database provider: dap`. If it does not, the DHIS2 version is
   older than 2.41 or the `jar` is not on the classpath.

## Architecture

The plugin is intentionally thin:

- `Dhis2PluginConfig` parses `analytics.database.*` properties out of DHIS2's
  configuration.
- `DapAnalyticsClient` is the HTTP client (Java 17 `java.net.http.HttpClient`).
- `DapAnalyticsDatabase` is the DHIS2 service-provider implementation. It builds the
  DAP request envelope from DHIS2's internal query shape and lifts the response to
  DHIS2's result grid.
- `DapProvenance` captures the DAP provenance stamp so DHIS2 can surface staleness.

Zero DHIS2 runtime dependency is pulled into this module — the plugin ships against an
adapter interface defined here (`AnalyticsDatabaseProvider`) that the DHIS2 build wires
to its own analytics service provider via a shaded dependency. Keeps the plugin
buildable without a full DHIS2 source checkout.
