# ADR 0258 — Per-object GRANT SELECT

## Status

Accepted.

## Context

ADR 0251 landed coarse global capabilities (`READ`, `WRITE`,
`DDL`, `ADMIN`). That leaves an all-or-nothing choice at the
user level: either every table is readable or none is.
Real deployments need table-scoped SELECT grants.

## Decision

- New `ObjectCapability` enum in `user_catalog` with a single
  variant `Select` (future slices add Insert/Update/Delete).
- `UserRecord.table_grants: BTreeMap<TableId, BTreeSet<ObjectCapability>>`
  with serde-default empty.
- `UserRecord::allows_on_table(table, cap)` resolves the
  check: global `Capability::Read` implies `Select` on every
  table; otherwise the per-table grant map decides.
- `UserCatalog::grant_on_table` / `revoke_on_table` —
  idempotent, reject unknown users. Revoke of the last
  capability in a table's set removes the table from the
  map so introspection is clean.
- `PersistentUserCatalog` journals `GrantOnTable` /
  `RevokeOnTable` commands.
- SQL grammar extends the existing `GRANT`/`REVOKE` arms:
  - `GRANT SELECT ON <qual-table> TO <user>` /
    `REVOKE SELECT ON <qual-table> FROM <user>` dispatch to
    the per-table path. The dispatcher looks one word ahead:
    `GRANT SELECT` with an `ON` clause goes per-table;
    otherwise the ADR 0251 global path handles it.
  - Rejects on unknown user (`SqlError::User`) or unknown
    table (`SqlError::Resolution`).
- `AuthenticatedSession` snapshots the user's `table_grants`
  at session start. When `authz.allows(Read)` is false and
  the statement is a simple single-table SELECT, the session
  resolves the FROM target and checks for a per-table SELECT
  grant. If present, it elevates the AuthorizationContext
  with `Read` for this one call and dispatches. Otherwise
  the normal path's authz fast-fail rejects.

## Consequences

- Scope limits:
  - Only `SELECT` is covered. INSERT/UPDATE/DELETE on a
    table-scoped grant require matching `ObjectCapability`
    variants and matching session-level elevation — future
    slice.
  - Simple single-table SELECT only. JOINs, subqueries, and
    multi-table FROMs fall through to the global authz check
    (so they still require `Capability::Read`). Per-table
    authorization for joins requires checking every
    referenced table before dispatch — future slice.
  - Grants are snapshotted at session start, so a GRANT /
    REVOKE issued mid-session is not visible until the next
    `with_authenticated_session`. The ADR calls this out so
    callers rely on the invariant.
- Admin users keep bypassing per-table checks because they
  hold global `Read`. This preserves the bootstrap story.
- 12 cases in `tests/sql_grant_object.rs` cover: no-grant
  blocks SELECT, GRANT allows SELECT, one-table grant does
  not transfer to another, REVOKE takes effect on next
  session, unknown-user / unknown-table rejected, grants
  persist across reopen, admin bypasses per-table check,
  grant does not enable INSERT, GRANT / REVOKE are
  idempotent, snapshot shows per-table grants.
