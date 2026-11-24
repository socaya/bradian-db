# ADR 0251 — GRANT/REVOKE grammar and first-user bootstrap

## Status

Accepted.

## Context

ADR 0250 bound an authenticated principal to a session but
gave every user administrator capabilities as an explicit
placeholder. This ADR narrows that: users are stored with an
explicit capability set, capabilities are managed by SQL, and
the capability bundle in the `AuthenticatedSession` is read
from the user record.

## Decision

- `UserRecord` carries
  `capabilities: BTreeSet<bradian_core::Capability>`
  (serde default empty for forward compatibility).
- `UserCatalog::grant(name, cap)` and `revoke(name, cap)`
  are idempotent and reject unknown names.
- `UserCatalog::create_user` auto-grants every capability
  (Read / Write / Ddl / Admin) when the catalog is empty at
  call time (the "bootstrap user"). Subsequent users start
  with an empty set.
- `PersistentUserCatalog`:
  - `Command::CreateUser` now carries `capabilities`
    (serde default empty on replay of older events).
  - New `Command::Grant { name, capability }` and
    `Command::Revoke { name, capability }`.
  - `create_user` decides bootstrap vs. normal from the LIVE
    catalog's `is_empty()` under the mutex, so a reopen
    followed by a second `CREATE USER` never re-triggers
    bootstrap unless every previous user was dropped first.
- SQL grammar:
  - `GRANT <cap> TO <user>` → `PersistentUserCatalog::grant`.
  - `REVOKE <cap> FROM <user>` → `revoke`.
  - `<cap>` is one of `READ`, `WRITE`, `DDL`, `ADMIN`.
- `SqlEngine::with_authenticated_session` now derives the
  `AuthorizationContext` from `UserRecord::capabilities()`
  instead of hardcoding administrator.

## Consequences

- New users (after the bootstrap) cannot run any SQL until
  an administrator grants them capabilities. Existing
  callers that relied on every authenticated user being
  admin will see `SqlError::Authz` — this is intentional.
- Bootstrap applies to any `CREATE USER` against an empty
  user catalog, including after every user has been dropped.
  Tests pin this behavior.
- The capability model is still coarse (per-user, four
  verbs). Per-object GRANT (table-scoped SELECT / UPDATE),
  role aggregation, and role-chain enforcement are future
  slices.
- `bradian_core::Capability` now derives `Ord` + `PartialOrd`
  so it can be a `BTreeSet` key.
- 14 cases in `tests/sql_grant_revoke.rs` cover: first-user
  bootstrap grants admin, second user starts empty,
  empty-cap session fast-fails SQL, GRANT READ enables
  SELECT, WRITE without DDL blocks CREATE, GRANT DDL enables
  CREATE, REVOKE removes a capability, GRANT idempotent,
  REVOKE of absent capability idempotent, GRANT on unknown
  user rejected, GRANT on unknown capability rejected,
  capabilities persist across reopen, bootstrap applies only
  once across reopen, drop-all-then-create re-bootstraps.
