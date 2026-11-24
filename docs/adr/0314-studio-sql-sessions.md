# ADR 0314 — Studio-facing SQL sessions

## Status

Accepted. Replaces the 2026-11-24 skeleton with a **real
session registry bound to `bradian_relational::SqlEngine`
and a real idle-expiry sweep**.

## Context

The skeleton exposed `SessionRegistry::{open, get, close}`
as an in-memory book-keeping surface with no engine
binding and no notion of session expiry. Studio needs
sessions that actually execute SQL under a principal and
that evict themselves when idle.

## Decision

New shape:

```rust
pub struct SqlSessionRegistry { ... }
impl SqlSessionRegistry {
    pub fn new(engine: Arc<SqlEngine>, idle_timeout: Duration) -> Self;
    pub fn open(&self, principal: impl Into<String>, now: SystemTime) -> SqlSession;
    pub fn execute(&self, id: SessionId, sql: &str, now: SystemTime)
        -> SessionResult<SqlResultSet>;
    pub fn close(&self, id: SessionId) -> bool;
    pub fn sweep(&self, now: SystemTime) -> Vec<SessionId>;
    pub fn get(&self, id: SessionId) -> Option<SqlSession>;
}
```

Semantics:

- `open` records `opened_at = last_used = now` for the
  session's principal and returns a `SqlSession` ticket.
- `execute(id, sql, now)` takes the registry mutex, looks
  up the session, refuses `NotFound` if absent, refuses
  `Expired` and **evicts** the session if
  `now - last_used > idle_timeout`, otherwise advances
  `last_used = now` and runs `engine.execute(sql)`.
  Engine errors surface as `SessionError::Sql(SqlError)`;
  the session stays live after an engine error.
- `sweep(now)` evicts every session whose
  `now - last_used > idle_timeout` and returns the ID
  list so a caller can audit.
- The registry is `Clone` (backed by `Arc<Inner>`) so an
  axum handler can share it across requests without
  threading state through handles.

Clock injection — every time-sensitive method takes
`now: SystemTime` explicitly, which lets tests drive the
clock deterministically and HTTP handlers pass
`SystemTime::now()` at the request boundary.

## Scope

**In scope:**
- Real `Arc<SqlEngine>` binding; sessions execute against
  the shared engine.
- Idle-timeout enforcement on access (`execute`) and on
  background sweep (`sweep`).
- Eviction of expired sessions on access, so a later
  access sees `NotFound` not stale state.
- Shareable registry behind `Arc`.
- 7 unit tests: real SQL round-trip; idle expiration on
  next execute; `last_used` reset extends the window;
  sweep evicts only idle sessions and reports IDs;
  closed + unknown sessions rejected as `NotFound`;
  engine errors surfaced verbatim and the session stays
  alive; `Clone` shares state across registry handles.

**Out of scope** (dedicated follow-ups):
- HTTP routes that mint session tickets from the
  authenticated principal — those plug into the
  existing `routes::router` and `auth.rs` (ADR
  0247/0250 plumbing is already present).
- Background sweeper task on a tokio interval — a
  caller can do `tokio::spawn` + loop around
  `registry.sweep(SystemTime::now())`.
- Session-scoped transaction binding (long-lived BEGIN
  across HTTP requests). Each `execute` currently uses
  `SqlEngine::execute` which is single-statement.

## Consequences

- The 2026-11-24 "SKELETON" status on 0314 is retired.
- No new workspace deps.
- A follow-up slice wires `/api/sql/v1/sessions` /
  `/api/sql/v1/sessions/:id/execute` axum routes into
  the already-authenticated handler chain.
