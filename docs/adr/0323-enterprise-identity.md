# ADR 0323 — Enterprise identity: OIDC JWT verification (HS256)

## Status

Accepted. Closes G1-LOCAL-ACCESS's "enterprise identity"
qualification item on the shared-secret axis. Asymmetric
(RS256 / ES256 / Ed25519) verification + JWKS fetch
remain dedicated follow-ups.

## Context

The existing `TokenStore` (`bradian-security::token`)
holds **opaque bearer tokens** hashed in-memory with
BLAKE3 — fine for first-party bootstrap, insufficient for
enterprise SSO where a corporate IdP (Keycloak, Okta,
Azure AD, Google, Auth0) issues signed access tokens the
API needs to trust.

HS256 — HMAC-SHA256 with a shared secret — is the
conservative first slice: every mainstream IdP supports
it as a client-credential option, the implementation
needs no asymmetric crypto, and the full round-trip is
testable in-process with fixture tokens.

## Decision

New module `bradian_security::enterprise_identity`:

```rust
pub struct JwtHs256Verifier { ... }
impl JwtHs256Verifier {
    pub fn new(secret: impl Into<Vec<u8>>) -> Self;
    pub fn with_issuer(mut self, iss: impl Into<String>) -> Self;
    pub fn with_audience(mut self, aud: impl Into<String>) -> Self;
    pub fn with_clock_skew_secs(mut self, secs: u64) -> Self;
    pub fn verify(&self, token: &str)                      -> JwtResult<Verified>;
    pub fn verify_at(&self, token: &str, now: SystemTime)  -> JwtResult<Verified>;
}
pub struct Verified { header: JwtHeader, claims: Value, std: StandardClaims }
pub enum JwtError { Malformed, UnsupportedAlg, BadSignature, Expired, NotYetValid, Base64, Json }
```

Semantics:

- Parses the three `.`-separated segments; refuses more
  or fewer than three.
- Decodes each segment with URL-safe base64 (no padding).
- Enforces `alg: "HS256"` in the header; any other alg
  (including `none`) is a `UnsupportedAlg` refusal.
- Recomputes `HMAC-SHA256(secret, "header.payload")` and
  compares to the signature in **constant time** via
  the `subtle` crate.
- Enforces `exp` with a configurable `skew_secs` window
  (default 30 seconds).
- Enforces `nbf` with the same skew window.
- Optional `iss` / `aud` enforcement — audiences in an
  array match if any entry matches the expected value.

Clock is injected via `verify_at(now: SystemTime)` so
tests drive it deterministically.

## Scope

**In scope:**
- Real HMAC-SHA256 signature verification with
  constant-time compare.
- Real base64-url segment decoding.
- Standard claims (`iss`, `sub`, `aud`, `exp`, `nbf`,
  `iat`) exposed on `StandardClaims`; every other claim
  flows through on `Verified::claims` as
  `serde_json::Value`.
- Explicit refusal of `alg: none` and non-HS256 algs.
- Issuer and audience matching (string + array).
- Configurable clock skew.
- 9 unit tests covering: valid round-trip, tampered
  payload, wrong secret, expired past skew, not-yet-valid,
  `alg: none` rejected, malformed at each segment, iss/aud
  enforcement, audience-array matching.

**Out of scope** (dedicated follow-ups):
- Asymmetric algorithms (RS256 / ES256 / Ed25519) —
  needs an RSA / EC crate; shipping those is a
  dependency decision.
- JWKS fetch (`/.well-known/jwks.json`) + rotation.
- `/authorize` / `/token` code-flow helpers.
- SAML 2.0 / Kerberos / NTLM.
- A lookup from a verified JWT `sub` into the local
  `Principal` (ADR 0250 plumbing is where that lands).

## Consequences

- An HTTP handler can accept an `Authorization: Bearer
  <jwt>` header, call `JwtHs256Verifier::verify`, and
  reason about the enterprise principal before touching
  SQL — the wiring into `auth.rs` is a bounded follow-up.
- Three new workspace deps (`hmac`, `sha2`, `base64`) —
  all pure-Rust, part of the Rust-crypto ecosystem, no
  build.rs, no C-level dependencies.
- The 2026-11-24 "SKELETON" status does not apply —
  this is a real, verifiable implementation from day
  one; signature verification is cryptographic, not
  structural.
- G1-LOCAL-ACCESS on the enterprise-identity axis is
  closed for the shared-secret case. Asymmetric and
  JWKS are a dedicated follow-up.
