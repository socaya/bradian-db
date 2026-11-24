# ADR 0248 — Persistent UserCatalog

## Status

Accepted.

## Context

ADR 0247 landed the in-memory `UserCatalog` with argon2id.
Without persistence, every user vanishes on reopen — the
primitive isn't usable for actual authentication. This ADR
puts user mutations onto a WAL-backed journal so credentials
survive restart, replayed through the same `UserCatalog`
validators that the live instance uses.

## Decision

- New module `persistent_user_catalog` with
  `PersistentUserCatalog`, mirroring the `PersistentCatalog`
  pattern.
- Dedicated journal namespace: marker
  `BRADIAN-USERS-V1`, event prefix `user/`.
- Separate `RowStore` from `PersistentCatalog`. Cross-journal
  atomicity (e.g. `GRANT` referencing both a user and a
  table) is NOT yet designed and is called out explicitly in
  future ADRs that span both.
- `Command` enum: `CreateUser { name, password_hash }`,
  `DropUser { name }`, `SetPassword { name, password_hash }`,
  `SetActive { name, active }`. Argon2id hashing runs at
  commit, not replay — the PHC string is persisted in the
  event so reopens are deterministic and don't pay the KDF
  cost per user per boot.
- Two new crate-visible `UserCatalog` methods:
  `insert_hashed(name, phc, active)` and
  `replace_hash(name, phc)` — reused by journal replay and
  by the commit path's tentative-clone validation.
- `commit` applies the event to a tentative clone first;
  validation errors (`Duplicate`, `NotFound`, empty hash)
  never reach the WAL and the journal LSN does not advance.

## Consequences

- Credentials persist across reopen. `verify_password` after
  reopen matches the pre-restart behavior.
- The user journal lives beside the DDL journal but is
  independent of it. A user named in a future GRANT must be
  validated via a cross-journal check at commit time; this
  slice does not provide that primitive.
- Failed mutations do not advance the journal (duplicate
  create, drop unknown) — tests pin this behavior so a
  later refactor cannot silently regress to "journal-then-
  reject".
- 10 cases in `tests/persistent_user_catalog.rs` cover:
  create + persist + reopen + verify; wrong-password after
  reopen; drop persists; password rotation persists; active
  flag persists; duplicate create does not journal; drop
  unknown does not journal; snapshot reflects current state;
  many users (12) survive reopen; journal LSN advances
  monotonically.
