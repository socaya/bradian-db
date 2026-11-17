# ADR 0153 — SqlEngine access-policy wiring (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-17.** First engine-level
integration of the ADR 0123 row/column/tenant policy
primitives. Promotes `TableAccessPolicy` from a
standalone module to an observable filter on real SQL
execution.

## 1. Scope

Two edits in `crates/bradian-relational/src/sql.rs`:

1. **`SqlError::Policy(PolicyError)`** — new variant on
   the existing `SqlError` enum with `#[from]` so any
   `allows_row` / `allows` error (unknown column,
   null-tenant, non-string-tenant) propagates with `?`.
2. **`SqlEngine::execute_with_policy(sql, policy,
   authz)`** — new public method taking a
   `&TableAccessPolicy` + `&AuthorizationContext`.
   Routes through the shared `execute_impl(sql,
   &default, None, Some(policy), Some(authz))`.
3. **`execute_impl(sql, control, budget, policy,
   authz)`** — the private helper now threads the two
   optional references. The SELECT branch, after row
   admission + budget charging, filters every row
   through `policy.allows_row(&row, &schema_cols,
   authz)` before the projection step.

`execute` / `execute_as` / `execute_with_control` /
`execute_with_budget` signatures and behavior unchanged
— all route through `execute_impl` with
`policy = None, authz = None`.

## 2. Out of scope (follow-ups)

- **Column-level projection** — `TableAccessPolicy::
  project_row` is not applied yet. The SELECT path's
  existing `indices` projection honors the caller's
  `SELECT col1, col2` list; a future ADR can intersect
  with `ColumnLevelPolicy::readable` for a
  "hide-columns" story.
- **Write-side policy** — INSERT / UPDATE / DELETE
  don't consult the policy. ADR 0123 CLS is projection-
  only; writes inherit from `AuthorizationContext` via
  ADR 0098.
- **Session-based engines** — `HistorySqlEngine`,
  `DomainSqlEngine`, `MvccSqlEngine` still OPEN. Same
  pattern applies.
- **Policy storage in the catalog** — policies are
  passed per-call today. A follow-up ADR can define
  `CREATE POLICY` / catalog storage + automatic
  retrieval.
- **Composition with `WHERE` predicates** — the policy
  is applied *after* the WHERE key-predicate filter;
  tests cover the combined effect.

## 3. Compliance

New test file `sql_engine_policy.rs`:

- `allow_all_policy_returns_every_row` — baseline.
- `always_deny_row_policy_filters_everything`.
- `tenant_isolation_filters_other_tenants` — multi-
  row filter.
- `row_level_column_equals_filter` — single-row
  filter.
- `row_level_unknown_column_errors_through_sqlerror`
  — PolicyError round-trips through `SqlError::Policy`.
- `execute_without_policy_returns_every_row` — proves
  the no-policy path is unchanged.
- `where_clause_and_policy_both_apply` — WHERE +
  policy compose correctly.

7 new integration tests.

## 4. Honest status

**BRD-SEC-002 "row/column/tenant policies end-to-end"**
moves from FOUNDATION ONLY to **PARTIAL**: the base
`SqlEngine` SELECT path now consults the policy. The
three session-based engines, write-side authz, catalog
storage of policies, and CLS projection remain OPEN.
