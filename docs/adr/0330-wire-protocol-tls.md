# ADR 0330 — TLS-wrapped wire protocol

## Status

Accepted. Extends ADR 0316's plaintext tokio TCP wire
with a rustls TLS wrapping. Satisfies the "TLS wrapping
remains a follow-up" entry from 0316.

## Context

ADR 0316 ships a real tokio-TCP wire protocol with
length-prefixed JSON frames and a SqlEngine-bound server.
ADR 0283 ships rustls primitives + mTLS-capable
`ServerConfig`. The natural combination — accept TCP,
wrap in `TlsStream`, run the existing wire handshake over
the encrypted stream — was called out explicitly as the
0316 follow-up.

## Decision

Refactor + extend `bradian_serving::wire`:

1. `WireServer::handle_connection` becomes generic over
   `S: AsyncRead + AsyncWrite + Unpin`. The framing +
   dispatch logic is now the same for plaintext and TLS.
2. New `WireServer::serve_one_tls(listener, &TlsAcceptor)`
   accepts one TCP connection, runs the rustls handshake
   via the supplied acceptor, and dispatches through the
   generic handler.
3. `WireClient` becomes `WireClientT<S>` (generic type)
   with two public aliases:
   - `WireClient    = WireClientT<TcpStream>`
     (plaintext — unchanged API).
   - `WireTlsClient = WireClientT<TlsStream<TcpStream>>`
     (new TLS variant).
4. New `WireTlsClient::connect_tls(addr, server_name,
   &TlsConnector, principal)` dials, runs the client
   handshake, and starts the wire handshake over the
   encrypted stream.
5. New `WireError::Tls(String)` variant distinguishes
   TLS-handshake failures from framing / SQL / peer
   errors so an operator can tell what layer rejected.

The actual TLS config (certs, roots, mTLS-vs-server-only,
ALPN, SNI) stays the caller's business — the wire module
takes already-built `TlsAcceptor` / `TlsConnector`
instances and does not re-encode any policy decisions.

## Scope

**In scope:**
- `handle_connection` generic over `AsyncRead + AsyncWrite
  + Unpin`; TCP and TLS paths share one implementation.
- Server-side TLS acceptance via `TlsAcceptor::accept`.
- Client-side TLS dial via `TlsConnector::connect` with
  caller-supplied `ServerName`.
- Three new integration tests against real sockets at
  `127.0.0.1:0` using a per-test rcgen-generated
  self-signed cert chain:
  1. TLS SELECT round-trip through the engine.
  2. Client with an untrusted server cert gets a `Tls`
     verification error (and the server-side task
     surfaces the broken handshake, not a wire error).
  3. A plaintext client talking to a TLS server surfaces
     `WireError::Tls` on the server task rather than
     silently negotiating or hanging.

**Out of scope** (dedicated follow-ups):
- mTLS (client-cert required) — ADR 0284 ships the
  `ClientCertVerifier` plumbing; a `serve_one_mtls`
  entry that composes this slice with that verifier is
  a one-liner follow-up once the deployment story
  (`CA bundle` provisioning) is agreed.
- TLS session resumption, 0-RTT, and ALPN selection.
- Certificate rotation hot-reload — ADR 0283 ships the
  `TlsAcceptorState::reload_if_changed` already; wiring
  it into a long-lived `serve_loop` is a follow-up.
- Principal-from-cert extraction (the client-supplied
  `principal` field in `Hello` is still a bearer
  identity, not a certificate-derived one).

## Consequences

- An operator can run `WireServer` behind a rustls
  acceptor loaded from PEM via ADR 0283's
  `load_pem`, and clients can dial it encrypted with
  server-cert verification.
- `tokio-rustls 0.26` is now a direct dep of
  `bradian-serving`; rustls was already a workspace dep.
- 8 wire tests total (5 plaintext from 0316 + 3 new TLS);
  the generic-handler refactor leaves every existing
  behaviour unchanged (verified by the preserved
  plaintext tests passing without modification).
