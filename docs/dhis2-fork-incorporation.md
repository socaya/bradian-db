# DHIS2 fork incorporation

The DAP ↔ DHIS2 upstream patches land in two trees:

- **This repo** (`~/projects/msdcse-project/dap`) owns the engine, the HTTP contract,
  the UI, and the reference Java sources under
  [`apps/dhis2-integration/`](../apps/dhis2-integration/).
- **The DHIS2 fork** (`~/projects/vitalworks/dhis2/repos/dhis2-core`) owns the
  DHIS2-core patch: enum entry, SqlBuilder stubs, provider registration, config keys.

Keep the two in sync whenever the DAP contract changes.

## Status

| Step | Where | Branch | Done |
|---|---|---|---|
| 1. Add `AnalyticsDatabase.DAP` + SqlBuilder stubs + config keys to DHIS2 | DHIS2 fork | `feat/dap-analytics-backend` | ✅ |
| 2. Ship `DapAnalyticsManager` + `DapAnalyticsTableManager` into `dhis-service-analytics` | DHIS2 fork | future commit on same branch | pending |
| 3. Parity test diffing DAP vs PostgreSQL | DHIS2 fork | future commit | pending |
| 4. Open DHIS2 upstream PR referencing ADR 0005 | DHIS2 upstream | — | pending |

## Build the DHIS2 fork

```bash
cd ~/projects/vitalworks/dhis2/repos/dhis2-core
git checkout feat/dap-analytics-backend
cd dhis-2/dhis-support/dhis-support-sql
mvn test
# ⇒ Tests run: 178, Failures: 0, Errors: 0, Skipped: 0
```

## Smoke-test against a running DAP engine

1. Start DAP in a shell:
   ```bash
   cd ~/projects/msdcse-project/dap
   cargo run -p dap-admin --release -- serve --root /tmp/dap-data --bind 127.0.0.1:8080
   ```
2. Put DHIS2 on the DAP backend. In `dhis.conf`:
   ```properties
   analytics.database               = dap
   analytics.database.base_url      = http://127.0.0.1:8080
   analytics.database.auth          = basic
   analytics.database.auth.username = dhis2
   analytics.database.auth.password = s3cret
   analytics.database.provenance    = warn
   ```
3. Start DHIS2. Startup should log:
   ```
   Analytics database initialized: 'DAP', ...
   DAP analytics backend selected — bootstrap handled by DapHttpClient service
   ```

Analytics requests fall back to PostgreSQL output until step 2 above lands the HTTP
query client. Degraded behaviour matches Doris when its JDBC catalog is unreachable.

## Rebasing on DHIS2 upstream

The DHIS2 fork currently descends from `vw-pro-additions`; when DHIS2 upstream ships
the changes from `feat/dap-analytics-backend`, the fork can drop the DAP-local commit
and pull the upstream one.
