# ADR 0344 — JWKS cache supports Ed25519 (OKP) keys

## Status

Accepted. Closes the "EdDSA JWKS dispatch" follow-up from
ADR 0343. Completes the JWKS cache's alg coverage:
RSA + ECDSA + EdDSA.

## Context

ADR 0342 made `JwksCache` dispatch between RSA and EC
keys via the `JwkVerifyingKey` enum. ADR 0343 added the
`JwtEddsaVerifier` primitive. The last piece — cache
support for OKP (RFC 8037) keys — lets an IdP that
publishes Ed25519 public keys in its JWKS rotate through
the same `JwksCache::spawn_rotation` loop.

## Decision

Extend `JwkVerifyingKey` with an `Eddsa` variant. Add
`ed25519_public_key_from_jwk` that parses
`kty: "OKP"`, `crv: "Ed25519"`, `x` (32-byte base64url
raw public key) into an `ed25519_dalek::VerifyingKey`.

`JwksCache::load` now accepts `kty: "OKP"` alongside
`"RSA"` and `"EC"`. `JwksCache::verify` dispatches to
`JwtEddsaVerifier::new(ed_key).verify(token)` when the
cached key is `Eddsa`.

## Scope

**In scope:**
- Real OKP Ed25519 JWK parsing: `x` base64url decodes to
  exactly 32 bytes, converted to `VerifyingKey` via
  `from_bytes`.
- Cache enum extended + verifier dispatch extended.
- 4 new tests (22 total in `jwks`):
  1. OKP JWK round-trips to `VerifyingKey` (compared by
     raw 32-byte public key).
  2. Non-Ed25519 curve rejected (`Ed448` for example).
  3. Missing `x` or wrong-length `x` rejected.
  4. Mixed RSA + Ed25519 cache: Ed25519-signed token
     verifies through the EdDSA path.

**Out of scope** (dedicated follow-ups):
- Ed448 (OKP + `crv: "Ed448"`) — one more variant on
  the enum; waiting for first deployment need.
- X25519 key-agreement keys (OKP + `crv: "X25519"`) —
  not a signing alg, out of JWS scope.

## Consequences

- An operator with an IdP that publishes Ed25519 JWKs
  can plug the JWKS URL directly into
  `JwksCache::spawn_rotation` and tokens verify
  automatically.
- No new workspace deps — `ed25519-dalek` already
  pulled in by ADR 0343.
- 22 JWKS tests green under `cargo clippy --workspace
  --all-targets --locked -- -D warnings`.
