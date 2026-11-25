# ADR 0328 — Enterprise identity: OIDC JWT verification (RS256)

## Status

Accepted. Extends ADR 0323 (HS256 shared-secret) with
asymmetric RS256 — the algorithm every mainstream IdP
(Keycloak, Okta, Azure AD, Google) **defaults** to for
access tokens.

## Context

ADR 0323 covers the shared-secret case. Enterprise
deployments typically use asymmetric keys so the IdP can
publish its public key via `/.well-known/jwks.json` and
the API can verify tokens without ever holding the signing
secret. The algorithm the OIDC core spec names for this is
RS256 — RSA PKCS#1 v1.5 signature over SHA-256.

## Decision

Add `JwtRs256Verifier` to `bradian_security::enterprise_identity`:

```rust
pub struct JwtRs256Verifier { ... }
impl JwtRs256Verifier {
    pub fn new(key: RsaPublicKey) -> Self;
    pub fn from_public_key_pem(pem: &str) -> JwtResult<Self>;
    pub fn with_issuer(mut self, iss: impl Into<String>) -> Self;
    pub fn with_audience(mut self, aud: impl Into<String>) -> Self;
    pub fn with_clock_skew_secs(mut self, secs: u64) -> Self;
    pub fn verify(&self, token: &str)                     -> JwtResult<Verified>;
    pub fn verify_at(&self, token: &str, now: SystemTime) -> JwtResult<Verified>;
}
```

Semantics:

- Parses the three `.`-separated segments.
- Enforces `alg: "RS256"`; `HS256` / `none` / any other
  alg → `JwtError::UnsupportedAlg`.
- Reconstructs the signing input (`"header.payload"` as
  ASCII) and verifies the RSA-SHA256-PKCS#1 v1.5
  signature via the pure-Rust `rsa` crate.
- Enforces `exp` / `nbf` with configurable clock skew.
- Optionally enforces `iss` / `aud`.
- Loads the public key from an SPKI PEM block
  (`-----BEGIN PUBLIC KEY-----` — the format every IdP
  publishes under `/.well-known/jwks.json` after
  base64-decoding the JWK's `n` + `e`).

New error variant `JwtError::KeyParse(String)` surfaces
PEM / key-parse failures without conflating them with the
static-string `Malformed` variant.

## Scope

**In scope:**
- Real RSA-SHA256 PKCS#1 v1.5 signature verification via
  the pure-Rust `rsa` crate (no `ring`, no C
  dependencies).
- SPKI PEM public-key loading.
- 7 new unit tests covering: fixture-signed round-trip,
  tampered payload fails signature, wrong key fails
  signature, wrong alg in header rejected (HS256 in an
  RS256-expected verifier), expired past skew, iss/aud
  enforced, PEM load (success + malformed-PEM error).
- Fixture keypair generated deterministically from
  `ChaCha20Rng::seed_from_u64(..)` so test runs are
  reproducible without committing a private key to the
  repo.

**Out of scope** (dedicated follow-ups):
- ES256 / Ed25519 / PS256 — one algorithm at a time.
- JWKS fetch (`/.well-known/jwks.json`) and rotation.
  ADR 0328 is key-value-in-hand; the fetch slice adds
  `reqwest` + rotation semantics.
- `kid` header matching against a JWKS key set.
- JWE (encrypted JWT).

## Consequences

- Bradian deployments can plug into any OIDC IdP's
  default RS256 flow without an intermediate
  shared-secret negotiation.
- Three new workspace deps (`rsa`, `rand_chacha` for
  test fixtures; `rand_chacha` was already a workspace
  member) — all pure-Rust, no C deps.
- Combined with ADR 0323, the `enterprise_identity`
  module now covers the two algorithms that cover
  >99 % of OIDC deployments.
