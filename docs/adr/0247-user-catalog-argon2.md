# ADR 0247 — User catalog with argon2id password hashing

## Status

Accepted.

## Context

Master-SRS P0-4 flags enterprise access as a Gate 1 blocker.
Before this ADR there was no catalog of users and no
password-verification primitive. Without those, no subsequent
work on authentication, GRANT grammar, session principal
binding, or TLS mutual auth has a foundation to build on.

## Decision

- New module `user_catalog` with an in-memory `UserCatalog`
  keyed by canonical lowercase name.
- `UserRecord { name, password_hash, active }` — only an
  argon2id PHC-encoded hash is stored; the plaintext is
  taken by value through `&str` and never retained.
- New workspace deps:
  - `argon2 = "0.5"` (feature `std`) — the audited
    reference implementation.
  - `password-hash = "0.5"` (feature `alloc`) — PHC
    encoding and constant-time verification.
  - `rand = "0.8"` on `bradian-relational` for `OsRng` salt.
- Argon2id params wired explicitly to the OWASP 2024 baseline
  (m=19456 KiB, t=2, p=1) so future retunes land in an ADR
  rather than a silent default change.
- API: `create_user`, `drop_user`, `set_password`, `set_active`,
  `verify_password`, `contains`, `get`, `users()`, `len`,
  `is_empty`.
- `verify_password` surfaces a single `InvalidPassword` error
  for unknown users, inactive users, and wrong passwords, so
  the error shape does not leak user existence.

## Consequences

- Scope: in-memory only; no persistence, no SQL grammar, no
  GRANT/REVOKE, no session-principal binding. Each lands in a
  follow-up slice.
- Identifier shape matches `catalog::identifier()` so names
  round-trip without re-encoding when a future slice maps
  users to schema-level principals.
- Serde derives let a future persistence slice journal
  mutations alongside the generic catalog. No persistence or
  WAL yet.
- 15 cases in `tests/user_catalog.rs` cover: create/verify
  happy path, wrong-password rejection, unknown-user same
  error shape, case-insensitive names, duplicate rejected,
  invalid-name shapes rejected, empty-password rejected on
  create and set, drop/drop-unknown, password rotation,
  inactive cannot authenticate, identical-password unique
  salts, serde round-trip verifies, `users()` lexicographic
  ordering, stored hash is `$argon2id$` PHC.
