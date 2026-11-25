# ADR 0332 — Principal-from-cert mapping

## Status

Accepted. Closes the "principal-from-cert" follow-up
called out by ADR 0331 — completes the mTLS authn → authz
handoff.

## Context

ADR 0331 proves the mTLS wire protocol accepts/refuses
client certs correctly. What it DOESN'T do: tell the
application layer who the peer is. After rustls finishes
the handshake, the server has a verified cert chain (via
`TlsStream::get_ref().1.peer_certificates()`) but the
wire handler still uses the client-supplied `Hello`
`principal` field — a bearer identity, not a
cert-derived one.

For real deployments, the peer's identity should come
from the **verified** certificate, not from a string the
client types. The industry-standard shape is: extract the
leaf cert's Subject Common Name and map it to the local
`Principal` via an operator-controlled policy.

## Decision

New module `bradian_security::cert_principal`:

```rust
pub fn extract_common_name(cert_der: &[u8]) -> CertPrincipalResult<String>;

pub fn principal_from_peer_chain(
    cert_chain: &[impl AsRef<[u8]>],
    mapper:     impl FnOnce(&str) -> Option<Principal>,
) -> CertPrincipalResult<Principal>;

pub enum CertPrincipalError {
    EmptyChain, ParseError(String), MissingCn,
    InvalidCnEncoding, MapperRefused(String),
}
```

Semantics:

- `extract_common_name` X.509-parses the leaf DER and
  returns the first Subject-CN attribute as `String`.
  Parse failure → `ParseError`; well-formed cert with no
  CN → `MissingCn`; CN with non-UTF-8 bytes →
  `InvalidCnEncoding`.
- `principal_from_peer_chain(chain, mapper)` takes the
  slice rustls returns from `peer_certificates()`
  (leaf at index 0), extracts the leaf CN, and calls
  the caller-supplied mapper. The mapper returns
  `Option<Principal>`: `None` → `MapperRefused(cn)`,
  `Some(p)` → `p`.
- The mapper is a `FnOnce` so an operator can close over
  the local `UserCatalog` / role config and decide
  policy without the module taking a stance.

## Scope

**In scope:**
- Leaf-cert Subject-CN extraction via the `x509-parser`
  crate (pure Rust).
- Operator-controlled mapper closure.
- `Role::Administrator` / `Role::Analyst` / `Role::System`
  mapping shown in the tests (including an OU-scoped
  Analyst derived from a CN prefix — the practical
  pattern an operator with a flat issuance CA uses).
- 8 unit tests: CN extracted from rcgen fixture,
  malformed DER rejected, `MissingCn` wiring, mapper
  resolution, mapper refusal, empty chain rejected,
  CN-encoded OU scope, leaf-not-chain-tail is used.

**Out of scope** (dedicated follow-ups):
- SAN (Subject-Alt-Name) based mapping — operators who
  encode principal in SAN need a sibling helper
  `extract_san_dns_name`.
- Policy-constrained CA chains (RFC 5280 extended key
  usage, name constraints). rustls's
  `WebPkiClientVerifier` already enforces basic path
  validation; principal-level policy is orthogonal.
- OID-based attribute extraction beyond CN (e.g. OU=,
  O= from the DN).
- Hot-reload of the mapper — the mapper is a `FnOnce`
  per call; stateful mappers (e.g. backed by a
  `UserCatalog`) live in the caller.
- Wiring into `WireServer::serve_one_tls`: the
  composition slot is there (the server can hand the
  peer-cert chain to this module before dispatching
  each `Execute`), but changing the wire handler to
  take a `mapper` closure is a dedicated follow-up
  slice.

## Consequences

- An operator running mTLS can now derive the local
  `Principal` from the verified peer cert instead of
  trusting the client-supplied `Hello` `principal`.
- `x509-parser 0.16` is now a direct dep of
  `bradian-security`; pure Rust, no C deps.
- `rcgen` becomes a dev-dep of `bradian-security` (it
  was already a workspace member) for fixture certs
  in the unit tests.
- The authn → authz handoff story is complete at the
  primitive level: mTLS (0283 + 0330 + 0331) verifies
  the cert; this module turns the verified cert into
  a `Principal` the existing `authz` layer can enforce.
