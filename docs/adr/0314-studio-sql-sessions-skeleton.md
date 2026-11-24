# ADR 0314 — Studio UI backing real SQL sessions: skeleton

## Status

Accepted as a skeleton. Seventh slice of P2.

## Context

Studio currently drives analytical endpoints directly. The
P2 acceptance calls for Studio to open **real SQL
sessions** (BEGIN / execute / COMMIT or ROLLBACK). A
production implementation is Gate-sized: HTTP session
tokens, idle timeouts, revocation hooked to the UserCatalog,
session-bound audit entries.

## Decision

New module `bradian_serving::sql_session` with a typed
session surface:

```rust
pub struct SessionId(pub u64);
pub struct SessionTicket { pub id: SessionId, pub principal: String, pub opened_at: SystemTime }

pub struct SessionRegistry { ... }
impl SessionRegistry {
    pub fn open(&self, principal: impl Into<String>) -> SessionTicket;
    pub fn get (&self, id: SessionId) -> Option<SessionTicket>;
    pub fn close(&self, id: SessionId) -> bool;
    pub fn len (&self) -> usize;
}
```

Honest scope: in-memory session book-keeping only. No HTTP
routes wired, no idle expiry timer, no SQL statement
execution bound to the session — those arrive when the
session is plumbed through `routes::router` under a real
authentication token (ADR 0283 TLS + 0284 mTLS already
provide the transport).

## Consequences

- Studio and future wire-protocol driver (ADR 0316) can
  take a `SessionTicket` across the HTTP boundary.
- 2 unit tests: open issues monotonic IDs; close removes.
