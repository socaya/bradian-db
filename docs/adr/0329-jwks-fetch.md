# ADR 0329 — JWKS fetch + kid-indexed verification

## Status

Accepted. Closes the "JWKS fetch + rotation" follow-up
called out by ADR 0328.

## Context

ADR 0328 ships an RS256 verifier that holds an
`RsaPublicKey` in hand. Production enterprise deployments
don't hand out the public key directly — the IdP
publishes it at `/.well-known/jwks.json` and **rotates**
by publishing new `kid`-tagged keys while keeping old ones
for a grace period. A consumer that can't walk the JWKS
has to redeploy to accept a rotated key.

## Decision

New module `bradian_security::jwks`:

```rust
pub struct Jwk    { kty, kid, alg, use_, n, e }
pub struct JwkSet { keys: Vec<Jwk> }

pub fn rsa_public_key_from_jwk(jwk: &Jwk) -> JwksResult<RsaPublicKey>;

pub struct JwksCache { ... }   // Arc-backed
impl JwksCache {
    pub fn new(ttl: Duration) -> Self;
    pub fn load(&self, set: &JwkSet) -> JwksResult<()>;
    pub fn fetch_from(&self, client: &reqwest::Client, url: &str)
        -> impl Future<Output = JwksResult<()>>;
    pub fn get(&self, kid: &str) -> Option<RsaPublicKey>;
    pub fn verify(&self, token: &str) -> JwksResult<Verified>;
    pub fn is_fresh(&self) -> bool;
}
```

Semantics:

- `rsa_public_key_from_jwk` base64url-decodes `n` + `e`
  and constructs an `rsa::RsaPublicKey`. Non-RSA `kty`
  → `Malformed`; missing `n` or `e` → `Malformed`.
- `JwksCache::load` replaces the cache from an
  already-parsed `JwkSet`. Non-RSA keys are silently
  skipped (log-only); malformed RSA keys surface as
  error so operators notice a broken IdP response.
- `JwksCache::fetch_from(client, url)` GETs the JWKS
  over HTTPS via a caller-supplied `reqwest::Client`
  (so TLS config, proxies, timeouts are out of this
  module's business), checks HTTP status, parses, and
  calls `load`.
- `JwksCache::verify(token)` peeks at the token's `kid`
  header, looks up the matching key, and delegates to
  `JwtRs256Verifier`. If the token has no `kid` and the
  cache has **exactly one** unkeyed JWK, that key is
  used (single-key tenant convention); otherwise
  `MissingKid`.
- `is_fresh()` returns true for `ttl` seconds after the
  last successful `load` / `fetch_from`.

## Scope

**In scope:**
- Real `n` + `e` → `rsa::RsaPublicKey` conversion via
  `BigUint::from_bytes_be`.
- RWLock-guarded cache; cheap `Arc`-based clone.
- HTTPS fetch via `reqwest`, status-checked.
- TTL freshness signal.
- `kid`-indexed lookup with single-unkeyed-key
  fallback.
- 10 unit tests: JWK → RsaPublicKey round-trip, non-RSA
  rejected, missing `n`/`e` rejected, cache load + kid
  lookup, verify selects correct key by kid, forged-kid
  forgery fails signature, unknown-kid rejected, single
  unkeyed JWK used when token omits kid, ambiguous
  multi-unkeyed tenant rejects kid-less tokens, TTL
  freshness flip, empty-cache paths.
- No actual HTTP in the test suite — `fetch_from` is
  exercised indirectly by `load` because the only
  difference is the bytes source; HTTP wiring is a
  trivial `reqwest::get` and testing the entire reqwest
  surface is not this module's job.

**Out of scope** (dedicated follow-ups):
- Scheduled background rotation (`spawn_rotation_loop`
  calling `fetch_from` on an interval).
- ETag / Last-Modified conditional fetch so the IdP
  can return 304.
- Automatic fallback: on a `UnknownKid`, attempt a
  refresh-and-retry (requires passing a reqwest client
  into `verify`).
- OIDC discovery (`/.well-known/openid-configuration`)
  resolution from an issuer base URL.

## Consequences

- An API handler can hold a shared `JwksCache`, serve
  requests against it synchronously, and refresh it in
  the background — the standard JWKS consumer shape.
- `reqwest` + `tokio` now appear in `bradian-security`'s
  direct deps (they were already transitive through
  other crates).
- With ADR 0328 + this slice, Bradian can speak to any
  OIDC IdP that publishes a standard JWKS without any
  additional wiring.
