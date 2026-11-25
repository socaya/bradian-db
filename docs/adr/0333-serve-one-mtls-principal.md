# ADR 0333 — `serve_one_mtls`: wire-handler-bound cert principal

## Status

Accepted. Closes the one explicit follow-up left in
ADR 0332 — actually uses `cert_principal` in the wire
handler so an mTLS peer is identified by their verified
cert, not by the client-supplied `Hello.principal`.

## Context

ADR 0330 ships `WireServer::serve_one_tls` that accepts
any `TlsAcceptor`. ADR 0331 proves the mTLS composition.
ADR 0332 ships `principal_from_peer_chain`. None of them
actually wires the three together — the wire handler
still reads whatever string the client writes in
`Hello.principal`.

## Decision

New method:

```rust
pub async fn serve_one_mtls<M>(
    &self, listener: &TcpListener, acceptor: &TlsAcceptor, mapper: M,
) -> WireResult<Principal>
where
    M: FnOnce(&str) -> Option<Principal>;
```

Pipeline:

1. `listener.accept()` — plaintext TCP.
2. `acceptor.accept(stream)` — rustls server handshake.
3. `tls.get_ref().1.peer_certificates()` — extract the
   verified peer chain. If the chain is missing or
   empty, return `WireError::Auth("mTLS requires a peer
   certificate chain")`. This also catches the
   mis-wiring where a non-mTLS acceptor (built with
   `with_no_client_auth`) is plumbed into
   `serve_one_mtls` — the peer chain is `None` and the
   call fails closed BEFORE any wire bytes land.
4. `principal_from_peer_chain(chain, mapper)` — derive
   the `Principal`. Mapper refusal → `WireError::Auth`.
5. `handle_connection(tls)` — run the existing
   generic wire handler.
6. Return the derived `Principal` so the caller knows
   who was authenticated on that connection.

New `WireError::Auth(String)` variant distinguishes
peer-authentication failures from TLS handshake failures
(`Tls`) and wire framing failures (`Handshake` /
`FrameTooLarge`).

## Scope

**In scope:**
- Real end-to-end integration: mTLS → rustls verifies
  cert path → `cert_principal` extracts CN → mapper
  → `Principal` → dispatch.
- Three new integration tests on real sockets with
  per-test rcgen certs:
  1. Valid mTLS cert + matching mapper → SELECT
     round-trip + `Principal::id = "alice"`.
  2. Mapper refuses the CN → `WireError::Auth` on the
     server task, wire handshake never runs.
  3. Non-mTLS acceptor plumbed into `serve_one_mtls`
     (operator mis-wiring) → `WireError::Auth` with
     "peer certificate" context, fails closed before
     any wire bytes.

**Out of scope** (dedicated follow-ups):
- `serve_many_mtls` loop with shared mapper and
  per-connection `Principal` audit emission.
- Replacing the `Hello.principal` field entirely with
  a server-assigned session identity. Right now the
  `Hello.principal` string is still read but discarded
  by the handler; the caller's observable result is
  the returned `Principal`.
- Authz enforcement (gating `Execute` on the derived
  `Principal.role`). The handler currently runs SQL
  without consulting the `Principal`; wiring the
  `authz` layer in is the natural next slice.

## Consequences

- An operator can deploy a cert-authenticated wire
  endpoint and get a verified `Principal` back from
  one method call per connection, no additional
  plumbing.
- `bradian-serving` depends on `bradian-security`'s
  `Principal` + `principal_from_peer_chain` (already a
  workspace dep).
- 14 wire tests total (5 plaintext + 3 TLS + 3 mTLS
  handshake + 3 mTLS principal-derivation). Every
  refusal path distinguishes `Handshake` / `Tls` /
  `Auth` / `FrameTooLarge` so an operator diagnosing
  a failure knows which layer rejected.
