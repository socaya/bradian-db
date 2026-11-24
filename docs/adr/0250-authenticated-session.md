# ADR 0250 — Authenticated session principal

## Status

Accepted.

## Context

ADR 0249 gave SQL grammar to the user catalog. There was no
way to thread an authenticated identity through subsequent
statements — every call fell back to `execute`, which runs
without a principal, so the existing `AuthorizationContext`
plumbing (ADR 0098) never fired on the user-managed path.

## Decision

- New `AuthenticatedSession<'a>` type bound to a
  `bradian_core::AuthorizationContext`. Immutable for its
  lifetime.
- New `SqlEngine::with_authenticated_session(user, pw,
  callback)`:
  1. Verifies the password via `PersistentUserCatalog`.
  2. Resolves the stored canonical name
     (case-insensitive round-trip).
  3. Derives an `AuthorizationContext` from the user.
  4. Yields an `AuthenticatedSession` to the callback.
- `AuthenticatedSession::execute(sql)` dispatches through
  `SqlEngine::execute_as(&self.authz, sql)` so every
  statement goes through the existing authz fast-fail.
- `AuthenticatedSession::principal()` and `capabilities()`
  expose the bound identity.

## Consequences

- Every authenticated user receives
  `AuthorizationContext::administrator` capabilities for now.
  The next slice (GRANT / roles) narrows this; this ADR is
  explicit that it is a placeholder.
- Credential verification happens once per session; the
  callback can issue many statements without re-paying the
  argon2 KDF cost.
- Wrong password, unknown user, and inactive user all
  surface as `SqlError::User` — the user-catalog already
  mapped those to the same `InvalidPassword` error so no
  existence oracle leaks.
- Nested sessions are not supported — each call to
  `with_authenticated_session` opens a fresh scope.
- 9 cases in `tests/authenticated_session.rs` cover:
  verify + execute, wrong password rejected, unknown user
  rejected, inactive user rejected, canonical-lowercase
  principal, placeholder admin capabilities, full DML
  round-trip, error propagation, two sessions with distinct
  principals.
