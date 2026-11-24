# ADR 0281 — Role catalog

## Status

Accepted.

## Context

Global capabilities (ADR 0251) and per-object grants (ADRs
0258 / 0278 / 0279) were per-user. Multi-user deployments
need named grant bundles so an operator can create a
"reader" or "writer" role once and grant it to many users.

## Decision

- New `RoleRecord { name, capabilities, table_grants }` —
  same grant shape as a user, no identity material.
- `UserCatalog.roles: BTreeMap<String, RoleRecord>` plus
  `UserRecord.roles: BTreeSet<String>`.
- API additions:
  - `create_role(name, capabilities)` — idempotent when
    re-created with identical caps; rejects on mismatch.
  - `drop_role(name)` — removes from users; idempotent.
  - `grant_role(user, role)` / `revoke_role(user, role)`.
  - `role(name)`, `roles_iter()` for introspection.
  - `effective_capabilities(user)` and
    `effective_table_grants(user)` — unions.
- Persistent journal: `Command::CreateRole`, `DropRole`,
  `GrantRole`, `RevokeRole`.
- SQL grammar:
  - `CREATE ROLE <name> [WITH <cap>[, <cap>…]]`
  - `DROP ROLE <name>`
  - `GRANT ROLE <role> TO <user>`
  - `REVOKE ROLE <role> FROM <user>`
- `AuthenticatedSession::execute` now consults
  `effective_capabilities` / `effective_table_grants` per
  statement. Global caps granted via role elevate the
  session's `AuthorizationContext` for the call.

## Consequences

- Role caps are only GLOBAL capabilities in this slice.
  Per-table grants on a role (e.g. "reader on
  t.public.items") work via `effective_table_grants` but
  aren't yet reachable via SQL grammar — adding `GRANT
  SELECT ON <table> TO ROLE <role>` is a one-line
  extension. Omitted here to keep the slice tight.
- Role chains (role grants another role) are NOT supported.
- Dropping a role revokes it from every user; the
  revoke-from-all step is in-memory only (no cross-table
  journal entry) — if the journal persists a
  `Command::DropRole` but the process crashes mid-revoke on
  replay, the next open re-runs the drop and auto-revokes
  again. Deterministic.
- 8 cases in `tests/role_catalog.rs` cover: create with
  caps; GRANT ROLE + user gets access; REVOKE ROLE removes
  access; DROP ROLE revokes from everyone; grant unknown
  role rejected; drop unknown role idempotent; persist
  across reopen; multi-cap role.
