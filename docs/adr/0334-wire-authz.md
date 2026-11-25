# ADR 0334 — Authz enforcement in the wire handler

## Status

Accepted. Closes the "authz enforcement" follow-up from
ADR 0333. Completes the end-to-end pipeline:
**cert → principal → enforced authz → SQL**.

## Context

ADR 0333 derived a `Principal` from the mTLS cert but the
wire handler didn't consult it when dispatching `Execute`.
Any authenticated client could run anything the underlying
`SqlEngine` would run. That's missing the "authorization"
half of the authn/authz pipeline the security stack has
been building toward.

## Decision

Refactor `WireServer::handle_connection` into
`handle_connection_with_authz(stream, authz)` that takes
an `FnMut(&str) -> bool` predicate. The plain
`handle_connection` wraps it with `|_| true` so the
non-mTLS paths are unchanged.

New method `WireServer::serve_one_mtls_authz`:

```rust
pub async fn serve_one_mtls_authz<M, A>(
    &self, listener: &TcpListener, acceptor: &TlsAcceptor,
    mapper: M, authz: A,
) -> WireResult<Principal>
where
    M: FnOnce(&str) -> Option<Principal>,
    A: FnMut(&Principal, &str) -> bool;
```

Pipeline per Execute:
1. Deserialise the `Execute { sql }` frame.
2. Call `authz(&principal, &sql)`.
3. If `false`: reply `ErrorMsg { code: 3, message:
   "authorization denied for statement: {sql}" }` and
   **keep the session alive** so the client can try
   other statements.
4. If `true`: dispatch to `SqlEngine::execute` as before.

## Scope

**In scope:**
- Real authz gate on every `Execute`.
- `FnMut` over `(Principal, &str)` so the caller can
  carry local state (counters, rate limits, dynamic
  policy) across statements in one session.
- Denial responses keep the session alive; this matches
  the behaviour of real production databases where a
  permission error on one statement doesn't tear down
  the connection.
- 2 new tests on top of the 14 from ADR 0330 / 0331 /
  0333:
  - Administrator may SELECT + INSERT (round-trip proves
    both pass the predicate and the engine).
  - Analyst may SELECT, is denied on INSERT (ErrorMsg
    code 3), and can still SELECT afterwards (session
    survives the denial).

**Out of scope** (dedicated follow-ups):
- A deep integration with `bradian-security::authz`'s
  `PolicyContext` + `cache_key` — the wire predicate is
  deliberately a thin closure so the caller chooses
  which policy engine to consult. A canonical
  "role → predicate" helper is a small follow-up slice.
- Parsing SQL inside the predicate to extract targeted
  objects (tables, columns). Current predicate sees
  the raw statement string; column-level authz is a
  separate concern in `bradian-security::authz`.
- Statement-type-aware audit entries in the audit log.

## Consequences

- An operator wiring `serve_one_mtls_authz` into their
  deployment now gets a complete authn→authz pipeline
  with zero additional wire code.
- 16 wire tests total; every refusal path still
  layer-distinguishes (`Tls` / `Handshake` / `Auth` /
  `code: 3 authz denial`).
