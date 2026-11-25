# ADR 0331 — Mutual-TLS wire protocol

## Status

Accepted. Closes the "mTLS remains a follow-up" entry
from ADR 0330 by proving the composition of ADR 0283's
`WebPkiClientVerifier`-backed server config with
ADR 0330's generic `serve_one_tls`.

## Context

ADR 0283 ships `load_pem_with_client_ca`, which builds a
rustls `ServerConfig` with
`WebPkiClientVerifier::builder(client_ca).build()` —
i.e. the server requires a client certificate chaining to
the supplied CA bundle.

ADR 0330 ships `WireServer::serve_one_tls(listener,
&TlsAcceptor)`, which accepts ANY `TlsAcceptor` without
taking a stance on whether the underlying `ServerConfig`
requires client auth.

The composition is: supply the mTLS-configured
`TlsAcceptor` to `serve_one_tls`, and the server rejects
any client that does not present a trusted cert.

## Decision

**No new public API is required.** The mTLS story is a
*composition* of existing primitives:

- Server: `tls::load_pem_with_client_ca(cert, key, ca)` →
  `TlsAcceptor::from(server_config)` → `WireServer::
  serve_one_tls(listener, &acceptor)`.
- Client: `rustls::ClientConfig::builder().
  with_root_certificates(trust).
  with_client_auth_cert(client_chain, client_key)` →
  `TlsConnector::from(client_cfg)` → `WireTlsClient::
  connect_tls(addr, server_name, &connector, principal)`.

This ADR documents the composition and ships three
integration tests that prove the authenticated path and
the two refusal paths:

1. **`mtls_round_trip_accepts_valid_client_cert`** —
   client presents a cert that chains to the server's
   client-CA bundle; the SELECT round-trip succeeds.
2. **`mtls_server_rejects_client_without_a_cert`** —
   client connects with a `ClientConfig` built by
   `with_no_client_auth`; both sides surface
   `WireError::Tls` (client-side on connect, server-side
   on accept) without running the wire handshake.
3. **`mtls_server_rejects_client_cert_from_untrusted_ca`**
   — client presents a cert signed by a CA the server
   does not trust; both sides surface `WireError::Tls`.

## Scope

**In scope:**
- End-to-end validation of the mTLS composition using
  three rcgen-generated fixture CAs per test (self-signed
  server cert, self-signed client cert, and in the
  untrusted-CA test a third self-signed cert the server
  is told NOT to trust).
- Per-test `rustls::crypto::ring::default_provider().
  install_default()` so the test suite remains
  order-independent.

**Out of scope** (dedicated follow-ups):
- Principal derivation from the verified client
  certificate (currently `principal` is still the
  bearer-identity string the client passes in `Hello`).
  Mapping a cert Subject-DN → `Principal` is a
  `bradian-security::principal` extension.
- CRL / OCSP revocation checking (rustls does neither
  by default; a follow-up adds a revocation hook).
- Certificate rotation hot-reload on the mTLS server
  (ADR 0283's `TlsAcceptorState::reload_if_changed` is
  the plumbing; wiring it into a long-lived serve-loop
  is the same slice as the ADR 0330 non-mTLS rotation
  follow-up).

## Consequences

- An operator can run `WireServer` with an mTLS
  `ServerConfig` and get real cryptographic client
  authentication with zero additional wire code.
- 11 wire tests total (5 plaintext + 3 TLS + 3 mTLS);
  every refusal path is exercised and distinguishes
  `WireError::Tls` from the wire-layer `Handshake` /
  `FrameTooLarge` / etc. errors.
- The 2026-11-24 "SKELETON" lineage is now fully gone
  from the wire module; it is a real plaintext + TLS +
  mTLS transport backed by `rustls` and the `SqlEngine`.
