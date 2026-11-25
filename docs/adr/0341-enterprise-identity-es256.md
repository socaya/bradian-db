# ADR 0341 — Enterprise identity: OIDC JWT verification (ES256)

## Status

Accepted. Extends ADR 0323 (HS256) and ADR 0328 (RS256)
with the third mainstream OIDC signature algorithm —
ECDSA on P-256, used by Apple and many Google
client-credentials flows as their default asymmetric alg.

## Context

ES256 is the ECDSA-SHA256 signature algorithm the OIDC
core spec lists alongside RS256 as acceptable
asymmetric options. It is widely used because EC keys
are much smaller than RSA at equivalent security (256-bit
vs 2048-bit) and signing is faster. A verifier that
supports HS256 + RS256 but not ES256 cannot talk to
every mainstream IdP.

## Decision

Add `JwtEs256Verifier` to
`bradian_security::enterprise_identity`:

```rust
pub struct JwtEs256Verifier { ... }
impl JwtEs256Verifier {
    pub fn new(key: p256::ecdsa::VerifyingKey) -> Self;
    pub fn from_public_key_pem(pem: &str) -> JwtResult<Self>;
    pub fn with_issuer(mut self, iss: impl Into<String>) -> Self;
    pub fn with_audience(mut self, aud: impl Into<String>) -> Self;
    pub fn with_clock_skew_secs(mut self, secs: u64) -> Self;
    pub fn verify(&self, token: &str)                     -> JwtResult<Verified>;
    pub fn verify_at(&self, token: &str, now: SystemTime) -> JwtResult<Verified>;
}
```

Semantics:

- Parses the three JWT segments; enforces `alg: "ES256"`.
- JWS ES256 signatures are the raw 64-byte r||s IEEE-P1363
  concatenation (NOT DER-wrapped, unlike generic ECDSA).
  The `p256` crate's `Signature::from_slice` consumes
  this shape directly.
- Verifies ECDSA signature over `"header.payload"` ASCII
  bytes using the standard `signature::Verifier` trait.
- Enforces `exp` / `nbf` with configurable clock skew.
- Optional `iss` / `aud` enforcement (same contract as
  HS256 / RS256).
- Loads the public key from SPKI PEM (the format every
  IdP publishes).

## Scope

**In scope:**
- Real ECDSA P-256 SHA-256 verification via the pure-Rust
  `p256` crate.
- SPKI PEM public-key loading.
- 6 new tests covering: fixture-signed round-trip,
  tampered payload fails signature, wrong key fails
  signature, wrong alg in header rejected (RS256 in an
  ES256-expected verifier), SPKI PEM load succeeds + a
  malformed PEM gives `KeyParse`, iss/aud enforcement.
- Fixture keypair generated deterministically from
  `ChaCha20Rng::seed_from_u64` so test runs are
  reproducible without committing an EC private key.

**Out of scope** (dedicated follow-ups):
- ES384 / ES512 (NIST P-384 / P-521 curves).
- EdDSA (Ed25519).
- Elliptic-curve JWK entries in `JwksCache` — the cache
  currently indexes RSA keys only. Extending it to
  dispatch RSA / EC by `kty` + `crv` is a one-slice
  follow-up once a deployment actually needs the
  multi-alg cache.

## Consequences

- `bradian-security` now supports every mainstream OIDC
  alg (HS256, RS256, ES256) at the verifier primitive
  level. Any IdP configured for any of these three can
  be consumed.
- `p256` 0.13 joins the workspace dep set — pure Rust,
  no C deps.
- 22 tests in the `enterprise_identity` module (16
  pre-existing + 6 new ES256). Workspace stays green
  under `cargo clippy --workspace --all-targets --locked
  -- -D warnings`.
