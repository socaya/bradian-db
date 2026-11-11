# ADR 0005 — DAP as a native DHIS2 analytics backend (Doris v43 model)

**Status:** Accepted · revised 2026-11-11
**Spec sections:** §1, §8, §10 · requirements DHI-001..004, API-001
**Supersedes:** the earlier revision of this ADR that proposed a service-loader plugin.

## Context

DHIS2 lets operators swap the analytics backend without changing application code. Up
to v42 the only choice was the in-process PostgreSQL build; v43 added Apache Doris as a
**first-class, upstream-native** backend. Operators select it with a single property:

```properties
analytics.database = doris
```

The Doris implementation is **not a plugin**. DHIS2 v43 shipped the Doris classes
directly in the DHIS2 source tree (`org.hisp.dhis.analytics.doris.*`): a `DorisAnalytics
Manager`, a `DorisAnalyticsTableManager`, an entry in the `AnalyticsDatabase` enum, and
the matching wiring in `AnalyticsManagerFactory` / `AnalyticsTableManagerFactory` /
`DhisConfigurationProvider`. Operators do not install a JAR; the DHIS2 release itself
contains the implementation.

## Decision

Mirror the v43 Doris pattern. DAP becomes a native DHIS2 analytics backend by landing
source files inside DHIS2 upstream — not a plugin, not a JAR to copy, not a service-
loader descriptor.

Repo layout:

```
apps/dhis2-integration/              <- the sources that land in DHIS2 upstream
  src/main/java/
    org/hisp/dhis/analytics/
      AnalyticsDatabase.java         # local copy of DHIS2's enum, with DAP added
      AnalyticsManager.java          # local copy of DHIS2's interface
      AnalyticsTableManager.java     # local copy of DHIS2's interface
    org/hisp/dhis/analytics/dap/     # new package DHIS2 adds under its tree
      DapAnalyticsBackend.java       # single entry-point the factories dispatch to
      DapConfiguration.java          # typed read of analytics.database.* properties
      DapHttpClient.java             # java.net.http.HttpClient wrapper
      DapAnalyticsManager.java       # implements AnalyticsManager
      DapAnalyticsTableManager.java  # implements AnalyticsTableManager
      DapRequestEnvelope.java        # wire shape, mirrors the Rust struct
      DapResponseEnvelope.java       # wire shape, mirrors the Rust struct
      DapIngestBatch.java            # batch of canonical changes, posted during rebuild
```

The local stand-ins for the three DHIS2 interfaces (`AnalyticsDatabase`,
`AnalyticsManager`, `AnalyticsTableManager`) exist only so this directory is buildable
outside a DHIS2 checkout. When the PR lands, the DHIS2 build swaps them for the
authoritative copies.

## PR plan

1. Open a DHIS2 issue requesting a new `AnalyticsDatabase.DAP` entry. Reference this
   ADR and the integration directory.
2. Rebase the sources onto a DHIS2 fork under:
   - `dhis-2/dhis-api/src/main/java/org/hisp/dhis/analytics/AnalyticsDatabase.java`
     (add the enum constant)
   - `dhis-2/dhis-services/dhis-service-analytics/src/main/java/org/hisp/dhis/analytics/dap/`
     (every other file)
3. Add DAP to DHIS2's existing factories so `analytics.database = dap` resolves to
   `DapAnalyticsBackend`. Mirror the Doris wiring in `AnalyticsManagerFactory` and
   `AnalyticsTableManagerFactory`.
4. Add a DHIS2 parity test that runs reference queries against DAP and diffs the
   result against DHIS2's PostgreSQL backend.
5. Submit the PR with ADR 0005 attached.

Until the PR lands, operators building their own DHIS2 image can apply these files as
a source patch — but there is no runtime plugin to install.

## Compatibility

- **DHIS2 2.41+**. 2.41 introduced the `AnalyticsDatabase` abstraction; DAP targets 2.41
  as the minimum. Earlier DHIS2 versions hard-wire PostgreSQL and cannot accommodate the
  enum extension.
- **Tested matrix:** 2.41, 2.42, 2.43 (where Doris is production-pinned). 2.44+ accepted
  pending SPI check.
- **Java 17** minimum, matching DHIS2 2.41's JDK requirement.

## Runtime shape

### Query path

DHIS2 call → `AnalyticsManagerFactory.get("dap")` → `DapAnalyticsManager.aggregate(...)`
→ `DapHttpClient.postAnalytics("/api/analytics.json", envelope)` → DAP serves the
response from `.derc` segments + delta → JSON envelope back to DHIS2 → DHIS2 lifts rows
into its `Grid` → user sees the result.

### Rebuild path

Nightly at `analytics.rebuild.cron` →
`AnalyticsTableManagerFactory.get("dap").prepareRebuild()` → DHIS2 streams rows out of
the operational PostgreSQL in batches of `analytics.database.ingest.batch_rows` →
`ingestBatch(...)` POSTs each batch to `/api/analytics/v1/ingest` → DAP applies via the
same canonical-change path as any other source (`dap-ingest`), with version fences +
quarantine → `finaliseRebuild()` → the engine flushes + compacts on its own schedule.

## Guarantees DAP keeps for DHIS2

Unchanged from the previous revision of this ADR:

1. Correct aggregation-type roll-ups (DHI-001).
2. Full DHIS2 period semantics (daily / weekly variants / bi-weekly / monthly /
   bi-monthly / quarterly / six-monthly / six-monthly-April / yearly / all four
   financial-year variants).
3. Reporting rates (`#{dsUid.REPORTING_RATE}` and friends).
4. Fail-closed on unsupported constructs.
5. Provenance on every response.

## `dhis.conf` properties

```properties
analytics.database               = dap
analytics.database.base_url      = https://dap.internal:8443
analytics.database.auth          = basic
analytics.database.auth.username = dhis2
analytics.database.auth.password = ${DAP_PASSWORD}

# Rebuild knobs (analytics-table build path).
analytics.database.ingest.batch_rows       = 10000
analytics.database.ingest.max_concurrency  = 4

# Timeouts.
analytics.database.connect_timeout_ms = 5000
analytics.database.read_timeout_ms    = 60000
analytics.database.retry.max_attempts = 3

# Provenance mode: strict / warn / lenient. Mirrors how DHIS2 v43 treats Doris
# availability — "warn" is the production-default.
analytics.database.provenance = warn

# Optional: pin the logical partition prefix DAP uses for this DHIS2 instance.
analytics.database.partition_prefix = p
```

See `docs/dhis.conf.example` for a full reference.

## Not implemented (deliberately)

- **JDBC/ODBC wire protocol.** DHIS2 uses it for Doris because Doris speaks MySQL
  natively; DAP is HTTP-first. The sources in `apps/dhis2-integration/` carry the full
  HTTP translation so DHIS2 does not need JDBC access to DAP.
- **Arbitrary SQL endpoints.** Spec §11 forbids them.
- **`/api/analytics/rawData.json`.** Unbounded line listings do not fit DAP's bounded
  export contract. Use the drilldown endpoint with an explicit limit.
