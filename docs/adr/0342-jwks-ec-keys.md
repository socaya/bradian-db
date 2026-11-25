# ADR 0342 — JWKS cache supports EC (ES256) keys

## Status

Accepted. Completes the ES256 story (ADR 0341) by letting
`JwksCache` serve `kty: "EC"` entries alongside RSA keys.
Closes the "JWKS EC-key dispatch" follow-up from ADR 0341.

## Context

ADR 0329 shipped a JWKS cache limited to RSA keys. ADR
0341 added an ES256 verifier but the cache couldn't hold
EC keys, so a deployment whose IdP publishes EC keys in
its JWKS couldn't use rotation. This slice unifies the
cache to serve both algorithms.

## Decision

Extend the `Jwk` struct with the EC-specific fields
`crv`, `x`, `y`. Add `p256_public_key_from_jwk` that
converts a `kty: "EC", crv: "P-256"` JWK into a
`p256::ecdsa::VerifyingKey`.

New cache-side enum:

```rust
pub enum JwkVerifyingKey {
    Rsa(RsaPublicKey),
    Es256(p256::ecdsa::VerifyingKey),
}

impl JwkVerifyingKey {
    pub fn from_jwk(jwk: &Jwk) -> JwksResult<Self>;
}
```

`JwksCache::load` now accepts both `kty: "RSA"` and
`kty: "EC"` (and silently skips unknown `kty` values —
unchanged behaviour on the ignore side).
`JwksCache::get(kid)` returns `JwkVerifyingKey`.
`JwksCache::verify(token)` dispatches to
`JwtRs256Verifier` or `JwtEs256Verifier` based on the
cached key variant; both verifiers enforce their own
`alg` on the token header, so a token with
`alg: "ES256"` against an RSA cache key is rejected as
`UnsupportedAlg` (and vice versa).

## Scope

**In scope:**
- EC P-256 JWK parsing: `x` + `y` base64url, each
  decoded to 32 bytes, converted to an uncompressed
  SEC1 point and then to `p256::ecdsa::VerifyingKey`.
- Cache enum + verifier dispatch.
- 5 new tests (18 total in `jwks`):
  1. EC JWK round-trips to `VerifyingKey` (using
     `VerifyingKey::PartialEq`).
  2. Non-P-256 curve rejected.
  3. Missing `x` or `y` rejected.
  4. Mixed-alg JWKS: cache accepts both RSA and EC
     entries; each token verifies against its own kid's
     key.
  5. Mismatched signature: RS256 token with an EC key's
     kid is rejected with `UnsupportedAlg` (alg check
     fires before the signature check).

**Out of scope** (dedicated follow-ups):
- P-384 / P-521 curves (ES384 / ES512).
- EdDSA (Ed25519 JWK with `kty: "OKP", crv: "Ed25519"`).
- Alg-aware preference when multiple keys share a kid
  — current cache uses last-write-wins on the `HashMap`
  insert, which the JWK spec forbids anyway (duplicate
  kids are a bug).

## Consequences

- An operator with an IdP that mixes RSA and EC signing
  keys can plug the JWKS URL into `JwksCache::fetch_from`
  / `spawn_rotation` without any client-side alg
  branching.
- `Jwk` grows 3 optional fields; serde `default = None`
  means existing RSA-only JWKs still deserialise.
- 18 JWKS tests, 51 `enterprise_identity` tests all
  green. Workspace stays green under `cargo clippy
  --workspace --all-targets --locked -- -D warnings`.
