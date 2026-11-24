# ADR 0249 — SQL grammar for user management

## Status

Accepted.

## Context

ADR 0248 landed `PersistentUserCatalog` but it was reachable
only from Rust callers. Users expect to manage accounts from
SQL. This ADR wires the user catalog through three new
statements and surfaces `verify_password` on `SqlEngine`.

## Decision

- `SqlEngine` now owns a `PersistentUserCatalog` opened at
  `<root>/users`. Open failures surface as `SqlError::Table`.
- New `SqlError::User(PersistentUserCatalogError)` variant
  for user-catalog errors.
- New statements:
  - `CREATE USER <name> WITH PASSWORD '<pw>'` →
    `PersistentUserCatalog::create_user`. Rejects a non-text
    password literal with `SqlError::Syntax`.
  - `DROP USER <name>` → `drop_user`.
  - `ALTER USER <name> SET PASSWORD '<pw>'` → `set_password`.
  - `ALTER USER <name> SET ACTIVE { TRUE | FALSE }` →
    `set_active`. Any other token after `ACTIVE` returns
    `SqlError::Syntax`.
- `SqlEngine::verify_password(user, password)` public API;
  Ok(()) only for known active users with matching password.
- `SqlEngine::user_catalog_snapshot()` returns a clone of
  the in-memory state for diagnostics / tests.

## Consequences

- User management is now a first-class SQL surface. Session
  principal binding, GRANT/REVOKE grammar, and authz
  enforcement remain separate slices.
- Case-insensitive names: identifier handling matches the
  catalog's `identifier()` rule, so `Alice` / `ALICE` /
  `alice` round-trip to the same stored user.
- Password literals must be single-quoted strings. Omitting
  quotes (number / word) fails at parse time; empty string
  fails at hash time with `InvalidPassword`.
- 12 cases in `tests/sql_user_grammar.rs` cover: create +
  verify, non-text password rejected, duplicate rejected,
  invalid / empty password rejected, drop + removes auth,
  drop unknown rejected, SET PASSWORD rotates, SET ACTIVE
  toggles, SET ACTIVE rejects non-boolean, case-insensitive
  names, reopen persists state, snapshot reflects users.
