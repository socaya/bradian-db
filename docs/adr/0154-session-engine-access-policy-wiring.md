# ADR 0154 — Session-engine access-policy wiring (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-17.** Extends ADR 0153
from the base `SqlEngine` to all three session-based
engines (`HistorySqlEngine`, `DomainSqlEngine`,
`MvccSqlEngine`). Completes SELECT-path policy coverage
across every current SQL engine.

## 1. Scope

Three narrow edits per engine:

1. Two new optional fields on each session struct:
   `policy: Option<TableAccessPolicy>` and
   `authz: Option<AuthorizationContext>`.
2. New constructor method: `with_session_policy(policy,
   authz, callback)` opens a session with the two
   fields populated. Existing `with_session*`
   constructors initialize both to `None`, preserving
   their pre-ADR behavior exactly.
3. SELECT-path instrumentation: after the existing
   budget charge, filter every row via
   `policy.allows_row(&row, &schema_cols, authz)` when
   both fields are `Some`.

`SqlError::Policy(PolicyError)` (added in ADR 0153)
absorbs any `allows_row` error via `?`.

## 2. Out of scope (follow-ups)

- **Column-level projection** — `TableAccessPolicy::
  project_row` not applied yet; follow-up ADR.
- **Write-side policy** — INSERT / UPDATE / DELETE
  don't consult the policy; writes inherit from
  `AuthorizationContext` via ADR 0098.
- **Catalog storage of policies** — policies are
  passed per-session; a future ADR defines `CREATE
  POLICY` + catalog retrieval.
- **Policy + budget composition** — the current API
  offers either `with_session_budget` OR
  `with_session_policy`; a combined constructor is a
  follow-up.

## 3. Compliance

New test file `session_engine_policy.rs` (7 tests):

- **DomainSqlEngine**: tenant isolation filters other
  tenants; AlwaysDeny strips everything; PolicyError
  surfaces via `SqlError::Policy`.
- **MvccSqlEngine**: tenant isolation hit; no-policy
  session unchanged.
- **HistorySqlEngine**: AlwaysDeny strips rows;
  no-policy session unchanged.

All three engines compile with the new fields and the
`with_session` / `with_session_budget` /
`with_session_budget_and_cancellation` existing paths
pass their test suites unchanged.

## 4. Honest status

**BRD-SEC-002 "row/column/tenant policies end-to-end"**
moves from PARTIAL (base engine only) to **PARTIAL (all
four engines on SELECT read paths)**. Column-level
projection, write-side authz, catalog storage and
policy+budget composition remain OPEN per §2.
