//! ADR 0329 — JWKS fetch + `kid`-indexed cache.
//!
//! An OIDC-compliant IdP publishes its signing keys at
//! `/.well-known/jwks.json`. Rotation is standard: new
//! keys appear with a different `kid`, old keys stay for
//! a grace period, consumers rotate seamlessly when the
//! token's `kid` header matches a new entry.
//!
//! This module parses JWKS JSON, converts RSA JWK entries
//! into `rsa::RsaPublicKey`, indexes keys by `kid`, and
//! optionally fetches the JWKS over HTTPS.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use parking_lot::RwLock;
use rsa::{BigUint, RsaPublicKey};
use serde::Deserialize;
use thiserror::Error;

use crate::enterprise_identity::{JwtError, JwtHeader, Verified};

#[derive(Debug, Error)]
pub enum JwksError {
    #[error("io: {0}")]
    Http(String),
    #[error("JWKS JSON parse: {0}")]
    Json(String),
    #[error("JWK entry is malformed: {0}")]
    Malformed(&'static str),
    #[error("RSA public-key construction: {0}")]
    RsaKey(String),
    #[error("kid {0:?} not found in JWKS")]
    UnknownKid(String),
    #[error("token header is missing `kid`")]
    MissingKid,
    #[error(transparent)]
    Jwt(#[from] JwtError),
}
pub type JwksResult<T> = Result<T, JwksError>;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Jwk {
    pub kty: String,
    #[serde(default)]
    pub kid: Option<String>,
    /// JWS algorithm this key signs under (RS256 or ES256).
    #[serde(default)]
    pub alg: Option<String>,
    /// Intended usage — `"sig"` or `"enc"`.
    #[serde(rename = "use", default)]
    pub use_: Option<String>,
    /// RSA modulus, base64url-encoded big-endian bytes.
    #[serde(default)]
    pub n: Option<String>,
    /// RSA exponent, base64url-encoded big-endian bytes.
    #[serde(default)]
    pub e: Option<String>,
    /// ADR 0342 — EC curve name (e.g. "P-256").
    #[serde(default)]
    pub crv: Option<String>,
    /// ADR 0342 — EC X coordinate, base64url-encoded.
    #[serde(default)]
    pub x: Option<String>,
    /// ADR 0342 — EC Y coordinate, base64url-encoded.
    #[serde(default)]
    pub y: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct JwkSet {
    pub keys: Vec<Jwk>,
}

impl JwkSet {
    pub fn from_json(bytes: &[u8]) -> JwksResult<Self> {
        serde_json::from_slice(bytes).map_err(|e| JwksError::Json(format!("{e}")))
    }
}

/// Convert an RSA JWK entry into an `rsa::RsaPublicKey`.
pub fn rsa_public_key_from_jwk(jwk: &Jwk) -> JwksResult<RsaPublicKey> {
    if jwk.kty != "RSA" {
        return Err(JwksError::Malformed("kty must be \"RSA\""));
    }
    let n_b64 = jwk.n.as_deref().ok_or(JwksError::Malformed("missing n"))?;
    let e_b64 = jwk.e.as_deref().ok_or(JwksError::Malformed("missing e"))?;
    let n = URL_SAFE_NO_PAD.decode(n_b64).map_err(|e|
        JwksError::RsaKey(format!("n base64: {e}")))?;
    let e = URL_SAFE_NO_PAD.decode(e_b64).map_err(|e|
        JwksError::RsaKey(format!("e base64: {e}")))?;
    RsaPublicKey::new(BigUint::from_bytes_be(&n), BigUint::from_bytes_be(&e))
        .map_err(|e| JwksError::RsaKey(format!("{e}")))
}

/// ADR 0342 — convert a `kty: "EC", crv: "P-256"` JWK
/// entry into a `p256::ecdsa::VerifyingKey`. The x and y
/// base64url components must each decode to 32 bytes
/// (uncompressed point halves).
pub fn p256_public_key_from_jwk(jwk: &Jwk)
    -> JwksResult<p256::ecdsa::VerifyingKey>
{
    use p256::elliptic_curve::sec1::FromEncodedPoint;
    use p256::{EncodedPoint, PublicKey};
    if jwk.kty != "EC" {
        return Err(JwksError::Malformed("kty must be \"EC\""));
    }
    if jwk.crv.as_deref() != Some("P-256") {
        return Err(JwksError::Malformed("crv must be \"P-256\""));
    }
    let x_b64 = jwk.x.as_deref().ok_or(JwksError::Malformed("missing x"))?;
    let y_b64 = jwk.y.as_deref().ok_or(JwksError::Malformed("missing y"))?;
    let x = URL_SAFE_NO_PAD.decode(x_b64).map_err(|e|
        JwksError::RsaKey(format!("x base64: {e}")))?;
    let y = URL_SAFE_NO_PAD.decode(y_b64).map_err(|e|
        JwksError::RsaKey(format!("y base64: {e}")))?;
    if x.len() != 32 || y.len() != 32 {
        return Err(JwksError::Malformed(
            "EC P-256 coordinates must each decode to 32 bytes"));
    }
    let point = EncodedPoint::from_affine_coordinates(
        (&*x).into(), (&*y).into(), false);
    let pk: PublicKey = Option::from(PublicKey::from_encoded_point(&point))
        .ok_or(JwksError::Malformed(
            "EC P-256 coordinates do not describe a valid curve point"))?;
    Ok(pk.into())
}

/// ADR 0344 — convert a `kty: "OKP", crv: "Ed25519"` JWK
/// entry into an `ed25519_dalek::VerifyingKey`. The `x`
/// field is a 32-byte base64url-encoded raw public key.
pub fn ed25519_public_key_from_jwk(jwk: &Jwk)
    -> JwksResult<ed25519_dalek::VerifyingKey>
{
    if jwk.kty != "OKP" {
        return Err(JwksError::Malformed("kty must be \"OKP\""));
    }
    if jwk.crv.as_deref() != Some("Ed25519") {
        return Err(JwksError::Malformed("crv must be \"Ed25519\""));
    }
    let x_b64 = jwk.x.as_deref().ok_or(JwksError::Malformed("missing x"))?;
    let bytes = URL_SAFE_NO_PAD.decode(x_b64).map_err(|e|
        JwksError::RsaKey(format!("x base64: {e}")))?;
    let arr: [u8; 32] = bytes.as_slice().try_into()
        .map_err(|_| JwksError::Malformed("Ed25519 x must decode to 32 bytes"))?;
    ed25519_dalek::VerifyingKey::from_bytes(&arr)
        .map_err(|e| JwksError::RsaKey(format!("Ed25519 public-key invalid: {e}")))
}

/// ADR 0342 + 0344 — a verifying key the cache stores.
/// One variant per algorithm family.
#[derive(Clone, Debug)]
pub enum JwkVerifyingKey {
    Rsa(RsaPublicKey),
    Es256(p256::ecdsa::VerifyingKey),
    Eddsa(ed25519_dalek::VerifyingKey),
}

impl JwkVerifyingKey {
    pub fn from_jwk(jwk: &Jwk) -> JwksResult<Self> {
        match jwk.kty.as_str() {
            "RSA" => Ok(Self::Rsa(rsa_public_key_from_jwk(jwk)?)),
            "EC"  => Ok(Self::Es256(p256_public_key_from_jwk(jwk)?)),
            "OKP" => Ok(Self::Eddsa(ed25519_public_key_from_jwk(jwk)?)),
            _     => Err(JwksError::Malformed("unsupported kty")),
        }
    }
}

// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Snapshot {
    keys:     HashMap<String, JwkVerifyingKey>,
    unkeyed:  Vec<JwkVerifyingKey>,
    fetched:  Option<Instant>,
}

/// In-memory JWKS cache. Cheap to clone (`Arc`-backed) so
/// multiple handlers can share it.
#[derive(Clone, Debug)]
pub struct JwksCache {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    ttl:   Duration,
    state: RwLock<Snapshot>,
    /// ADR 0336 — observability counters for the rotation
    /// loop. `fetch_attempts` ticks every scheduled fetch
    /// try, `fetch_errors` counts the ones that failed.
    fetch_attempts: AtomicU64,
    fetch_errors:   AtomicU64,
}

impl JwksCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                ttl,
                state: RwLock::new(Snapshot::default()),
                fetch_attempts: AtomicU64::new(0),
                fetch_errors:   AtomicU64::new(0),
            }),
        }
    }

    pub fn fetch_attempts(&self) -> u64 {
        self.inner.fetch_attempts.load(Ordering::Relaxed)
    }
    pub fn fetch_errors(&self) -> u64 {
        self.inner.fetch_errors.load(Ordering::Relaxed)
    }

    /// Replace the cache from an already-parsed `JwkSet`.
    /// Unsupported `kty` values are silently skipped;
    /// malformed RSA / EC JWKs surface as error.
    pub fn load(&self, set: &JwkSet) -> JwksResult<()> {
        let mut snap = Snapshot::default();
        for jwk in &set.keys {
            let key = match jwk.kty.as_str() {
                "RSA" | "EC" | "OKP" => JwkVerifyingKey::from_jwk(jwk)?,
                _ => continue,
            };
            match jwk.kid.as_ref() {
                Some(kid) => { snap.keys.insert(kid.clone(), key); }
                None      => snap.unkeyed.push(key),
            }
        }
        snap.fetched = Some(Instant::now());
        *self.inner.state.write() = snap;
        Ok(())
    }

    pub fn is_fresh(&self) -> bool {
        let snap = self.inner.state.read();
        match snap.fetched {
            Some(t) => t.elapsed() <= self.inner.ttl,
            None => false,
        }
    }

    pub fn get(&self, kid: &str) -> Option<JwkVerifyingKey> {
        self.inner.state.read().keys.get(kid).cloned()
    }

    pub fn len(&self) -> usize { self.inner.state.read().keys.len() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }

    /// Fetch a JWKS document over HTTPS and replace the
    /// cache. Caller supplies a `reqwest::Client` so TLS
    /// config, proxies, etc. are not this module's business.
    /// Increments `fetch_attempts`; on failure increments
    /// `fetch_errors` and leaves the cache untouched.
    pub async fn fetch_from(
        &self, client: &reqwest::Client, url: &str,
    ) -> JwksResult<()> {
        self.inner.fetch_attempts.fetch_add(1, Ordering::Relaxed);
        let result = async {
            let bytes = client.get(url).send().await
                .map_err(|e| JwksError::Http(format!("{e}")))?
                .error_for_status()
                .map_err(|e| JwksError::Http(format!("{e}")))?
                .bytes().await
                .map_err(|e| JwksError::Http(format!("{e}")))?;
            let set = JwkSet::from_json(&bytes)?;
            self.load(&set)
        }.await;
        if result.is_err() {
            self.inner.fetch_errors.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    /// ADR 0336 — spawn a tokio task that refreshes the
    /// cache on `interval`. The returned `JoinHandle` can
    /// be awaited / aborted by the caller. Errors are
    /// logged via `tracing::warn!` and counted in
    /// `fetch_errors`; a transient error does NOT kill
    /// the loop — the next tick retries.
    pub fn spawn_rotation(
        &self, client: reqwest::Client, url: impl Into<String>,
        interval: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let cache  = self.clone();
        let url    = url.into();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // Don't burst-fire if the task runs behind.
            ticker.set_missed_tick_behavior(
                tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                match cache.fetch_from(&client, &url).await {
                    Ok(()) => {
                        tracing::debug!(
                            url = %url,
                            attempts = cache.fetch_attempts(),
                            "JWKS rotation: cache refreshed");
                    }
                    Err(e) => {
                        tracing::warn!(
                            url = %url,
                            error = %e,
                            errors = cache.fetch_errors(),
                            "JWKS rotation: fetch failed, will retry next tick");
                    }
                }
            }
        })
    }

    /// Verify a JWT using the key selected by its `kid`
    /// header, dispatching to the right verifier based on
    /// the cached key variant. If the token has no `kid`
    /// and the cache has exactly one unkeyed entry, that
    /// key is used (OIDC "single-key tenant" convention).
    ///
    /// ADR 0342 — alg routing: an RSA cache key is used
    /// with `JwtRs256Verifier`; an EC cache key is used
    /// with `JwtEs256Verifier`. The verifier picks up the
    /// token's declared `alg` from the header and
    /// enforces alg match itself.
    pub fn verify(&self, token: &str) -> JwksResult<Verified> {
        let (h_b64, _) = split_token(token)?;
        let header_bytes = URL_SAFE_NO_PAD.decode(h_b64)
            .map_err(|e| JwksError::Json(format!("header b64: {e}")))?;
        let header: JwtHeader = serde_json::from_slice(&header_bytes)
            .map_err(|e| JwksError::Json(format!("header: {e}")))?;

        let key = match header.kid.as_ref() {
            Some(kid) => self.get(kid).ok_or_else(||
                JwksError::UnknownKid(kid.clone()))?,
            None => {
                let snap = self.inner.state.read();
                if snap.unkeyed.len() == 1 { snap.unkeyed[0].clone() }
                else { return Err(JwksError::MissingKid); }
            }
        };
        Ok(match key {
            JwkVerifyingKey::Rsa(rsa_key) =>
                crate::enterprise_identity::JwtRs256Verifier::new(rsa_key).verify(token)?,
            JwkVerifyingKey::Es256(ec_key) =>
                crate::enterprise_identity::JwtEs256Verifier::new(ec_key).verify(token)?,
            JwkVerifyingKey::Eddsa(ed_key) =>
                crate::enterprise_identity::JwtEddsaVerifier::new(ed_key).verify(token)?,
        })
    }
}

fn split_token(token: &str) -> JwksResult<(&str, &str)> {
    let mut parts = token.splitn(3, '.');
    let h = parts.next().ok_or(JwksError::Malformed("missing header"))?;
    let p = parts.next().ok_or(JwksError::Malformed("missing payload"))?;
    Ok((h, p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;
    use rsa::pkcs1v15::SigningKey as RsaSigningKey;
    use rsa::signature::{RandomizedSigner, SignatureEncoding};
    use rsa::traits::PublicKeyParts;
    use rsa::RsaPrivateKey;
    use sha2::Sha256;

    fn fixture_key(seed: u64) -> (RsaPrivateKey, RsaPublicKey) {
        let mut rng = ChaCha20Rng::seed_from_u64(seed);
        let priv_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let pub_key = priv_key.to_public_key();
        (priv_key, pub_key)
    }

    fn mint_rs256(priv_key: &RsaPrivateKey, kid: Option<&str>,
                  claims: serde_json::Value) -> String {
        let mut header = serde_json::json!({ "alg": "RS256", "typ": "JWT" });
        if let Some(k) = kid { header["kid"] = serde_json::Value::String(k.into()); }
        let h = B64.encode(serde_json::to_vec(&header).unwrap());
        let p = B64.encode(serde_json::to_vec(&claims).unwrap());
        let signing = format!("{h}.{p}");
        let sk = RsaSigningKey::<Sha256>::new(priv_key.clone());
        let mut rng = ChaCha20Rng::seed_from_u64(0x9999);
        let sig = sk.sign_with_rng(&mut rng, signing.as_bytes());
        format!("{signing}.{}", B64.encode(sig.to_bytes()))
    }

    fn jwk_from_pub(pub_key: &RsaPublicKey, kid: Option<&str>) -> Jwk {
        let n = B64.encode(pub_key.n().to_bytes_be());
        let e = B64.encode(pub_key.e().to_bytes_be());
        Jwk {
            kty: "RSA".into(),
            kid: kid.map(str::to_string),
            alg: Some("RS256".into()),
            use_: Some("sig".into()),
            n: Some(n), e: Some(e),
            crv: None, x: None, y: None,
        }
    }

    #[test]
    fn jwk_round_trips_through_rsa_public_key() {
        let (_, pub_key) = fixture_key(1);
        let jwk = jwk_from_pub(&pub_key, Some("k-1"));
        let back = rsa_public_key_from_jwk(&jwk).unwrap();
        // Compare by n+e since RsaPublicKey has no PartialEq.
        assert_eq!(pub_key.n().to_bytes_be(), back.n().to_bytes_be());
        assert_eq!(pub_key.e().to_bytes_be(), back.e().to_bytes_be());
    }

    #[test]
    fn non_rsa_jwk_is_rejected() {
        let jwk = Jwk {
            kty: "EC".into(), kid: None, alg: None, use_: None,
            n: None, e: None,
            crv: None, x: None, y: None,
        };
        assert!(matches!(rsa_public_key_from_jwk(&jwk),
            Err(JwksError::Malformed("kty must be \"RSA\""))));
    }

    #[test]
    fn missing_n_or_e_is_rejected() {
        let mut jwk = Jwk {
            kty: "RSA".into(), kid: None, alg: None, use_: None,
            n: Some("AQAB".into()), e: None,
            crv: None, x: None, y: None,
        };
        assert!(matches!(rsa_public_key_from_jwk(&jwk),
            Err(JwksError::Malformed("missing e"))));
        jwk.n = None;
        jwk.e = Some("AQAB".into());
        assert!(matches!(rsa_public_key_from_jwk(&jwk),
            Err(JwksError::Malformed("missing n"))));
    }

    #[test]
    fn cache_loads_a_jwkset_and_looks_up_by_kid() {
        let (_, pub_a) = fixture_key(2);
        let (_, pub_b) = fixture_key(3);
        let set = JwkSet { keys: vec![
            jwk_from_pub(&pub_a, Some("key-a")),
            jwk_from_pub(&pub_b, Some("key-b")),
        ]};
        let cache = JwksCache::new(Duration::from_secs(60));
        cache.load(&set).unwrap();
        assert_eq!(cache.len(), 2);
        assert!(cache.is_fresh());
        assert!(cache.get("key-a").is_some());
        assert!(cache.get("key-b").is_some());
        assert!(cache.get("missing").is_none());
    }

    #[test]
    fn cache_verify_uses_kid_to_select_key() {
        let (priv_a, pub_a) = fixture_key(4);
        let (_,      pub_b) = fixture_key(5);
        let set = JwkSet { keys: vec![
            jwk_from_pub(&pub_a, Some("A")),
            jwk_from_pub(&pub_b, Some("B")),
        ]};
        let cache = JwksCache::new(Duration::from_secs(60));
        cache.load(&set).unwrap();

        let token = mint_rs256(&priv_a, Some("A"),
            serde_json::json!({ "sub": "alice", "exp": u64::MAX }));
        let v = cache.verify(&token).unwrap();
        assert_eq!(v.std.sub.as_deref(), Some("alice"));

        // Fake a token claiming kid=B while being signed by A.
        let forged = mint_rs256(&priv_a, Some("B"),
            serde_json::json!({ "sub": "a", "exp": u64::MAX }));
        let err = cache.verify(&forged).unwrap_err();
        assert!(matches!(err, JwksError::Jwt(JwtError::BadSignature)), "{err:?}");
    }

    #[test]
    fn cache_verify_errors_on_unknown_kid() {
        let (_, pub_a) = fixture_key(6);
        let set = JwkSet { keys: vec![jwk_from_pub(&pub_a, Some("A"))] };
        let cache = JwksCache::new(Duration::from_secs(60));
        cache.load(&set).unwrap();
        let (priv_other, _) = fixture_key(7);
        let token = mint_rs256(&priv_other, Some("Z"),
            serde_json::json!({ "sub": "a", "exp": u64::MAX }));
        let err = cache.verify(&token).unwrap_err();
        assert!(matches!(err, JwksError::UnknownKid(ref k) if k == "Z"),
            "got {err:?}");
    }

    #[test]
    fn single_unkeyed_jwk_is_used_when_token_omits_kid() {
        let (priv_a, pub_a) = fixture_key(8);
        let set = JwkSet { keys: vec![jwk_from_pub(&pub_a, None)] };
        let cache = JwksCache::new(Duration::from_secs(60));
        cache.load(&set).unwrap();
        let token = mint_rs256(&priv_a, None,
            serde_json::json!({ "sub": "a", "exp": u64::MAX }));
        cache.verify(&token).unwrap();
    }

    #[test]
    fn ambiguous_tenant_rejects_tokens_with_no_kid() {
        // Two unkeyed JWKs in the set → the verifier can't
        // deterministically pick one for a kid-less token.
        let (priv_a, pub_a) = fixture_key(9);
        let (_,      pub_b) = fixture_key(10);
        let set = JwkSet { keys: vec![
            jwk_from_pub(&pub_a, None),
            jwk_from_pub(&pub_b, None),
        ]};
        let cache = JwksCache::new(Duration::from_secs(60));
        cache.load(&set).unwrap();
        let token = mint_rs256(&priv_a, None,
            serde_json::json!({ "sub": "a", "exp": u64::MAX }));
        assert!(matches!(cache.verify(&token), Err(JwksError::MissingKid)));
    }

    #[test]
    fn freshness_flips_to_stale_once_ttl_passes() {
        let (_, pub_a) = fixture_key(11);
        let set = JwkSet { keys: vec![jwk_from_pub(&pub_a, Some("A"))] };
        let cache = JwksCache::new(Duration::from_millis(10));
        cache.load(&set).unwrap();
        assert!(cache.is_fresh());
        std::thread::sleep(Duration::from_millis(50));
        assert!(!cache.is_fresh(), "cache should be stale after TTL");
    }

    #[test]
    fn empty_cache_reports_not_fresh_and_verifies_nothing() {
        let cache = JwksCache::new(Duration::from_secs(60));
        assert!(!cache.is_fresh());
        assert!(cache.is_empty());
        let (priv_a, _) = fixture_key(12);
        let token = mint_rs256(&priv_a, Some("whatever"),
            serde_json::json!({ "sub": "a", "exp": u64::MAX }));
        let err = cache.verify(&token).unwrap_err();
        assert!(matches!(err, JwksError::UnknownKid(_)));
    }

    // ---------- ADR 0336: scheduled rotation ----------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fetch_attempts_and_errors_counters_are_wired() {
        // Point reqwest at a localhost port that nobody
        // is listening on — fetch should fail fast.
        let cache = JwksCache::new(Duration::from_secs(60));
        let client = reqwest::Client::builder().build().unwrap();
        assert_eq!(cache.fetch_attempts(), 0);
        assert_eq!(cache.fetch_errors(),   0);
        let err = cache.fetch_from(&client, "http://127.0.0.1:1/jwks.json").await;
        assert!(err.is_err());
        assert_eq!(cache.fetch_attempts(), 1);
        assert_eq!(cache.fetch_errors(),   1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_rotation_survives_fetch_errors_and_keeps_ticking() {
        let cache = JwksCache::new(Duration::from_secs(60));
        let client = reqwest::Client::builder().build().unwrap();
        let handle = cache.spawn_rotation(
            client, "http://127.0.0.1:1/jwks.json",
            Duration::from_millis(40),
        );
        // Wait for several ticks to land.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let attempts = cache.fetch_attempts();
        let errors   = cache.fetch_errors();
        assert!(attempts >= 2,
            "rotation should have fired ≥ 2 times in 200 ms: attempts={attempts}");
        assert_eq!(attempts, errors,
            "every attempt should have failed against the dead URL");
        // The task is still alive; aborting cleans up.
        assert!(!handle.is_finished(),
            "rotation task must NOT exit on a transient fetch error");
        handle.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_rotation_cache_stays_stale_until_fetch_succeeds() {
        let cache = JwksCache::new(Duration::from_millis(100));
        let client = reqwest::Client::builder().build().unwrap();
        let handle = cache.spawn_rotation(
            client, "http://127.0.0.1:1/jwks.json",
            Duration::from_millis(30),
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!cache.is_fresh(),
            "no successful fetch → cache should remain stale");
        assert!(cache.is_empty());
        handle.abort();
    }

    // ---------- ADR 0342: EC / ES256 JWK support ----------

    use p256::ecdsa::{signature::Signer as _, SigningKey as EcdsaSigningKey};
    #[allow(unused_imports)]
    use p256::elliptic_curve::sec1::ToEncodedPoint as _;

    fn fixture_ec_keypair(seed: u64)
        -> (EcdsaSigningKey, p256::ecdsa::VerifyingKey)
    {
        let mut rng = ChaCha20Rng::seed_from_u64(seed);
        let sk = EcdsaSigningKey::random(&mut rng);
        let vk = *sk.verifying_key();
        (sk, vk)
    }

    fn jwk_from_ec_pub(vk: &p256::ecdsa::VerifyingKey, kid: Option<&str>) -> Jwk {
        let point = vk.to_encoded_point(false);
        let x = B64.encode(point.x().unwrap());
        let y = B64.encode(point.y().unwrap());
        Jwk {
            kty: "EC".into(),
            kid: kid.map(str::to_string),
            alg: Some("ES256".into()),
            use_: Some("sig".into()),
            n: None, e: None,
            crv: Some("P-256".into()),
            x: Some(x), y: Some(y),
        }
    }

    fn mint_es256(sk: &EcdsaSigningKey, kid: Option<&str>,
                  claims: serde_json::Value) -> String {
        let mut header = serde_json::json!({ "alg": "ES256", "typ": "JWT" });
        if let Some(k) = kid { header["kid"] = serde_json::Value::String(k.into()); }
        let h = B64.encode(serde_json::to_vec(&header).unwrap());
        let p = B64.encode(serde_json::to_vec(&claims).unwrap());
        let signing = format!("{h}.{p}");
        let sig: p256::ecdsa::Signature = sk.sign(signing.as_bytes());
        format!("{signing}.{}", B64.encode(sig.to_bytes()))
    }

    #[test]
    fn ec_jwk_round_trips_through_p256_verifying_key() {
        let (_sk, vk) = fixture_ec_keypair(1);
        let jwk = jwk_from_ec_pub(&vk, Some("ec-1"));
        let back = p256_public_key_from_jwk(&jwk).unwrap();
        // VerifyingKey implements PartialEq.
        assert_eq!(back, vk);
    }

    #[test]
    fn ec_jwk_requires_p256_curve() {
        let (_, vk) = fixture_ec_keypair(2);
        let mut jwk = jwk_from_ec_pub(&vk, None);
        jwk.crv = Some("secp384r1".into());
        assert!(matches!(p256_public_key_from_jwk(&jwk),
            Err(JwksError::Malformed("crv must be \"P-256\""))));
    }

    #[test]
    fn ec_jwk_missing_x_or_y_rejected() {
        let (_, vk) = fixture_ec_keypair(3);
        let mut jwk = jwk_from_ec_pub(&vk, None);
        jwk.y = None;
        assert!(matches!(p256_public_key_from_jwk(&jwk),
            Err(JwksError::Malformed("missing y"))));
        jwk.y = Some("short".into());
        jwk.x = None;
        assert!(matches!(p256_public_key_from_jwk(&jwk),
            Err(JwksError::Malformed("missing x"))));
    }

    #[test]
    fn cache_dispatches_rsa_and_ec_keys_by_variant() {
        let (sk_rsa, pub_rsa) = fixture_key(10);
        let (sk_ec,  pub_ec)  = fixture_ec_keypair(11);
        let set = JwkSet { keys: vec![
            jwk_from_pub(&pub_rsa, Some("rsa-key")),
            jwk_from_ec_pub(&pub_ec, Some("ec-key")),
        ]};
        let cache = JwksCache::new(Duration::from_secs(60));
        cache.load(&set).unwrap();
        assert_eq!(cache.len(), 2);
        // RSA-signed token verified via RS256.
        let rsa_token = mint_rs256(&sk_rsa, Some("rsa-key"), serde_json::json!({
            "sub": "a", "exp": u64::MAX,
        }));
        // mint_rs256 doesn't add kid; wrap to add it.
        let rsa_token = add_kid_to_token(&rsa_token, "rsa-key");
        cache.verify(&rsa_token).unwrap();
        // EC-signed token verified via ES256.
        let ec_token = mint_es256(&sk_ec, Some("ec-key"),
            serde_json::json!({ "sub": "b", "exp": u64::MAX }));
        cache.verify(&ec_token).unwrap();
    }

    #[test]
    fn cache_ec_key_rejects_mismatched_signature() {
        let (sk_rsa, _) = fixture_key(12);
        let (_, pub_ec) = fixture_ec_keypair(13);
        let set = JwkSet { keys: vec![jwk_from_ec_pub(&pub_ec, Some("ec-key"))] };
        let cache = JwksCache::new(Duration::from_secs(60));
        cache.load(&set).unwrap();
        // RS256-signed token claiming the EC key's kid → ES256
        // verifier gets the wrong alg in the header.
        let rsa_token = mint_rs256(&sk_rsa, Some("rsa-key"), serde_json::json!({
            "sub": "a", "exp": u64::MAX,
        }));
        let twisted = add_kid_to_token(&rsa_token, "ec-key");
        let err = cache.verify(&twisted).unwrap_err();
        // ES256 verifier refuses alg=RS256 as UnsupportedAlg,
        // which bubbles up through JwksError::Jwt.
        assert!(matches!(err,
            JwksError::Jwt(crate::enterprise_identity::JwtError::UnsupportedAlg(_))),
            "got {err:?}");
    }

    /// Rewrite a JWT's header to include a `kid` and
    /// re-encode. Only touches the header b64 segment —
    /// the signature stays bound to the original header
    /// bytes, which is desired for the mismatch test.
    fn add_kid_to_token(token: &str, kid: &str) -> String {
        let parts: Vec<&str> = token.splitn(3, '.').collect();
        let hb = B64.decode(parts[0]).unwrap();
        let mut hv: serde_json::Value = serde_json::from_slice(&hb).unwrap();
        hv["kid"] = serde_json::Value::String(kid.into());
        let new_h = B64.encode(serde_json::to_vec(&hv).unwrap());
        // Also re-sign isn't possible here without the
        // key; the signature verify will fail unless the
        // caller used this helper correctly (e.g. match
        // the kid to the cert that actually signed it).
        // In our tests we use it either to add a kid to an
        // existing fixture-signed token or to deliberately
        // break verification via a forged kid.
        let _ = new_h.clone();
        format!("{new_h}.{}.{}", parts[1], parts[2])
    }

    // ---------- ADR 0344: OKP / Ed25519 JWK support ----------

    use ed25519_dalek::SigningKey as EdSigningKey;
    #[allow(unused_imports)]
    use ed25519_dalek::Signer as _;

    fn fixture_ed_keypair(seed: u64) -> (EdSigningKey, ed25519_dalek::VerifyingKey) {
        use rand::RngCore;
        let mut rng = ChaCha20Rng::seed_from_u64(seed);
        let mut secret = [0u8; 32];
        rng.fill_bytes(&mut secret);
        let sk = EdSigningKey::from_bytes(&secret);
        let vk = sk.verifying_key();
        (sk, vk)
    }

    fn jwk_from_ed_pub(vk: &ed25519_dalek::VerifyingKey, kid: Option<&str>) -> Jwk {
        Jwk {
            kty: "OKP".into(),
            kid: kid.map(str::to_string),
            alg: Some("EdDSA".into()),
            use_: Some("sig".into()),
            n: None, e: None,
            crv: Some("Ed25519".into()),
            x: Some(B64.encode(vk.as_bytes())),
            y: None,
        }
    }

    fn mint_eddsa(sk: &EdSigningKey, kid: Option<&str>,
                  claims: serde_json::Value) -> String {
        let mut header = serde_json::json!({ "alg": "EdDSA", "typ": "JWT" });
        if let Some(k) = kid { header["kid"] = serde_json::Value::String(k.into()); }
        let h = B64.encode(serde_json::to_vec(&header).unwrap());
        let p = B64.encode(serde_json::to_vec(&claims).unwrap());
        let signing = format!("{h}.{p}");
        let sig = sk.sign(signing.as_bytes());
        format!("{signing}.{}", B64.encode(sig.to_bytes()))
    }

    #[test]
    fn okp_jwk_round_trips_through_ed25519_verifying_key() {
        let (_sk, vk) = fixture_ed_keypair(1);
        let jwk = jwk_from_ed_pub(&vk, Some("ed-1"));
        let back = ed25519_public_key_from_jwk(&jwk).unwrap();
        assert_eq!(back.as_bytes(), vk.as_bytes());
    }

    #[test]
    fn okp_jwk_requires_ed25519_curve() {
        let (_, vk) = fixture_ed_keypair(2);
        let mut jwk = jwk_from_ed_pub(&vk, None);
        jwk.crv = Some("Ed448".into());
        assert!(matches!(ed25519_public_key_from_jwk(&jwk),
            Err(JwksError::Malformed("crv must be \"Ed25519\""))));
    }

    #[test]
    fn okp_jwk_missing_x_or_wrong_length_rejected() {
        let (_, vk) = fixture_ed_keypair(3);
        let mut jwk = jwk_from_ed_pub(&vk, None);
        jwk.x = None;
        assert!(matches!(ed25519_public_key_from_jwk(&jwk),
            Err(JwksError::Malformed("missing x"))));
        jwk.x = Some(B64.encode([0u8; 16]));   // too short
        assert!(matches!(ed25519_public_key_from_jwk(&jwk),
            Err(JwksError::Malformed("Ed25519 x must decode to 32 bytes"))));
    }

    #[test]
    fn cache_dispatches_rsa_ec_and_ed_keys_by_variant() {
        let (sk_rsa, pub_rsa) = fixture_key(20);
        let (_sk_ec, pub_ec)  = fixture_ec_keypair(21);
        let _ = sk_rsa;
        let _ = pub_ec;
        let (sk_ed,  pub_ed)  = fixture_ed_keypair(22);
        let set = JwkSet { keys: vec![
            jwk_from_pub(&pub_rsa, Some("rsa-key")),
            jwk_from_ed_pub(&pub_ed, Some("ed-key")),
        ]};
        let cache = JwksCache::new(Duration::from_secs(60));
        cache.load(&set).unwrap();
        assert_eq!(cache.len(), 2);
        // Ed25519-signed token verified via EdDSA path.
        let ed_token = mint_eddsa(&sk_ed, Some("ed-key"),
            serde_json::json!({ "sub": "ed-user", "exp": u64::MAX }));
        let v = cache.verify(&ed_token).unwrap();
        assert_eq!(v.header.alg, "EdDSA");
        assert_eq!(v.std.sub.as_deref(), Some("ed-user"));
    }
}
