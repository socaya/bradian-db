# ADR 0343 — Enterprise identity: OIDC JWT verification (EdDSA / Ed25519)

## Status

Accepted. Completes the asymmetric algorithm trio
(RSA + ECDSA + EdDSA) by adding Ed25519 verification per
RFC 8037 (`alg: "EdDSA"`).

## Context

Ed25519 is the modern elliptic-curve signature used by
many next-gen IdPs (and by anyone who prefers a faster,
smaller alternative to RSA / P-256). RFC 8037 defines
`alg: "EdDSA"` as the JWS algorithm name for Ed25519.
A verifier that supports HS256 / RS256 / ES256 but not
EdDSA cannot talk to every mainstream IdP.

## Decision

Add `JwtEddsaVerifier` to
`bradian_security::enterprise_identity`:

```rust
pub struct JwtEddsaVerifier { ... }
impl JwtEddsaVerifier {
    pub fn new(key: ed25519_dalek::VerifyingKey) -> Self;
    pub fn from_public_key_pem(pem: &str) -> JwtResult<Self>;
    pub fn with_issuer(mut self, iss: impl Into<String>) -> Self;
    pub fn with_audience(mut self, aud: impl Into<String>) -> Self;
    pub fn with_clock_skew_secs(mut self, secs: u64) -> Self;
    pub fn verify   (&self, token: &str)                    -> JwtResult<Verified>;
    pub fn verify_at(&self, token: &str, now: SystemTime)   -> JwtResult<Verified>;
}
```

Semantics:

- Enforces `alg: "EdDSA"`; any other alg → `UnsupportedAlg`.
- JWS Ed25519 signatures are the 64-byte R||S shape that
  `ed25519_dalek::Signature::from_slice` consumes
  directly.
- Verifies via `ed25519_dalek::VerifyingKey::verify`.
- Same iss/aud/exp/nbf/skew contract as the HS256 /
  RS256 / ES256 verifiers.
- Loads public key from SPKI PEM.

## Scope

**In scope:**
- Real Ed25519 verification via the pure-Rust
  `ed25519-dalek` 2.x crate.
- SPKI PEM public-key loading.
- 6 new tests (28 total in `enterprise_identity`): fixture-
  signed round-trip, tampered payload rejected, wrong key
  rejected, ES256-alg in EdDSA-expected header rejected,
  SPKI PEM load + malformed-PEM gives `KeyParse`, iss/aud
  enforcement.
- Deterministic fixture keypair via
  `ChaCha20Rng::fill_bytes` + `SigningKey::from_bytes`.

**Out of scope** (dedicated follow-ups):
- Ed448 (`alg: "EdDSA"` with curve `Ed448`).
- EdDSA JWK support in `JwksCache` (`kty: "OKP"`,
  `crv: "Ed25519"`) — one-slice follow-up to extend
  the ADR 0342 dispatch enum with an `Eddsa` variant.

## Consequences

- `bradian-security` now covers every mainstream OIDC
  signature algorithm at the verifier primitive level:
  HS256 (0323) + RS256 (0328) + ES256 (0341) + EdDSA
  (this slice).
- `ed25519-dalek` 2.x joins the workspace dep set —
  pure Rust, no C deps.
- Workspace stays green under `cargo clippy --workspace
  --all-targets --locked -- -D warnings`.
