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
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use parking_lot::RwLock;
use rsa::{BigUint, RsaPublicKey};
use serde::Deserialize;
use thiserror::Error;

use crate::enterprise_identity::{
    JwtError, JwtHeader, JwtRs256Verifier, Verified,
};

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
    /// JWS algorithm this key signs under (RS256 for our
    /// purposes).
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

// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Snapshot {
    keys:     HashMap<String, RsaPublicKey>,
    unkeyed:  Vec<RsaPublicKey>,  // JWKs that omit `kid`
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
}

impl JwksCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                ttl, state: RwLock::new(Snapshot::default()),
            }),
        }
    }

    /// Replace the cache from an already-parsed `JwkSet`.
    /// Non-RSA keys are silently skipped (log-only
    /// diagnostic); malformed RSA JWKs surface as error.
    pub fn load(&self, set: &JwkSet) -> JwksResult<()> {
        let mut snap = Snapshot::default();
        for jwk in &set.keys {
            if jwk.kty != "RSA" { continue; }
            let key = rsa_public_key_from_jwk(jwk)?;
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

    pub fn get(&self, kid: &str) -> Option<RsaPublicKey> {
        self.inner.state.read().keys.get(kid).cloned()
    }

    pub fn len(&self) -> usize { self.inner.state.read().keys.len() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }

    /// Fetch a JWKS document over HTTPS and replace the
    /// cache. Caller supplies a `reqwest::Client` so TLS
    /// config, proxies, etc. are not this module's business.
    pub async fn fetch_from(
        &self, client: &reqwest::Client, url: &str,
    ) -> JwksResult<()> {
        let bytes = client.get(url).send().await
            .map_err(|e| JwksError::Http(format!("{e}")))?
            .error_for_status()
            .map_err(|e| JwksError::Http(format!("{e}")))?
            .bytes().await
            .map_err(|e| JwksError::Http(format!("{e}")))?;
        let set = JwkSet::from_json(&bytes)?;
        self.load(&set)
    }

    /// Verify a JWT using the key selected by its `kid`
    /// header. If the token has no `kid` and the cache
    /// has exactly one unkeyed entry, that key is used
    /// (OIDC "single-key tenant" convention).
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
        Ok(JwtRs256Verifier::new(key).verify(token)?)
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
        };
        assert!(matches!(rsa_public_key_from_jwk(&jwk),
            Err(JwksError::Malformed("kty must be \"RSA\""))));
    }

    #[test]
    fn missing_n_or_e_is_rejected() {
        let mut jwk = Jwk {
            kty: "RSA".into(), kid: None, alg: None, use_: None,
            n: Some("AQAB".into()), e: None,
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
}
