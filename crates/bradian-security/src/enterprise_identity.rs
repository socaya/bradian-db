//! Enterprise identity — OIDC JWT verification.
//!
//! ADR 0323 ships HS256 (shared-secret HMAC-SHA256);
//! ADR 0328 extends with RS256 (RSA-SHA256 PKCS#1 v1.5)
//! for IdPs that use asymmetric keys (Keycloak / Okta /
//! Azure AD / Google default).
//!
//! Each verifier parses a JWS compact token, enforces its
//! algorithm (refuses `none` and anything else), verifies
//! the signature, enforces `exp` / `nbf` with a
//! configurable clock skew, and optionally enforces
//! `iss` / `aud`.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use rsa::pkcs1v15::{Signature as RsaSignature, VerifyingKey as RsaVerifyingKey};
#[allow(unused_imports)]
use rsa::signature::Verifier as _;
use rsa::RsaPublicKey;
use serde::Deserialize;
use sha2::Sha256;
use subtle::ConstantTimeEq;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum JwtError {
    #[error("malformed JWT: {0}")]
    Malformed(&'static str),
    #[error("unsupported algorithm: {0}")]
    UnsupportedAlg(String),
    #[error("public-key parse error: {0}")]
    KeyParse(String),
    #[error("signature mismatch")]
    BadSignature,
    #[error("token expired: exp={exp}, now={now}")]
    Expired { exp: u64, now: u64 },
    #[error("token not yet valid: nbf={nbf}, now={now}")]
    NotYetValid { nbf: u64, now: u64 },
    #[error("base64 decode: {0}")]
    Base64(String),
    #[error("JSON decode: {0}")]
    Json(String),
}

pub type JwtResult<T> = Result<T, JwtError>;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct JwtHeader {
    pub alg: String,
    #[serde(default)]
    pub kid: Option<String>,
    #[serde(default)]
    pub typ: Option<String>,
}

/// Subset of the JWT claims this verifier enforces. All
/// other claims pass through on `Verified::claims`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct StandardClaims {
    #[serde(default)]
    pub iss: Option<String>,
    #[serde(default)]
    pub sub: Option<String>,
    #[serde(default)]
    pub aud: Option<serde_json::Value>,
    #[serde(default)]
    pub exp: Option<u64>,
    #[serde(default)]
    pub nbf: Option<u64>,
    #[serde(default)]
    pub iat: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Verified {
    pub header:  JwtHeader,
    pub claims:  serde_json::Value,
    pub std:     StandardClaims,
}

/// HS256 verifier. The secret is caller-supplied; this
/// slice does not take a stance on where it comes from
/// (file, secret manager, env var).
#[derive(Clone, Debug)]
pub struct JwtHs256Verifier {
    secret:   Vec<u8>,
    /// Optional expected `iss` — rejected if mismatched.
    expected_iss: Option<String>,
    /// Optional expected `aud` (string match). An array
    /// `aud` is accepted if any entry matches.
    expected_aud: Option<String>,
    /// Allowed clock-skew tolerance when comparing `exp`
    /// / `nbf` against `now`.
    skew_secs:    u64,
}

impl JwtHs256Verifier {
    pub fn new(secret: impl Into<Vec<u8>>) -> Self {
        Self {
            secret: secret.into(),
            expected_iss: None,
            expected_aud: None,
            skew_secs:    30,
        }
    }

    pub fn with_issuer(mut self, iss: impl Into<String>) -> Self {
        self.expected_iss = Some(iss.into()); self
    }
    pub fn with_audience(mut self, aud: impl Into<String>) -> Self {
        self.expected_aud = Some(aud.into()); self
    }
    pub fn with_clock_skew_secs(mut self, secs: u64) -> Self {
        self.skew_secs = secs; self
    }

    pub fn verify_at(&self, token: &str, now: SystemTime) -> JwtResult<Verified> {
        let mut parts = token.split('.');
        let h_b64 = parts.next().ok_or(JwtError::Malformed("missing header"))?;
        let p_b64 = parts.next().ok_or(JwtError::Malformed("missing payload"))?;
        let s_b64 = parts.next().ok_or(JwtError::Malformed("missing signature"))?;
        if parts.next().is_some() {
            return Err(JwtError::Malformed("extra segments after signature"));
        }

        let header_bytes  = decode_b64(h_b64)?;
        let payload_bytes = decode_b64(p_b64)?;
        let signature     = decode_b64(s_b64)?;

        let header: JwtHeader = serde_json::from_slice(&header_bytes)
            .map_err(|e| JwtError::Json(format!("header: {e}")))?;
        if header.alg != "HS256" {
            return Err(JwtError::UnsupportedAlg(header.alg.clone()));
        }

        // Recompute HMAC over the signing input and
        // constant-time compare against the signature.
        let signing_input = format!("{h_b64}.{p_b64}");
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&self.secret)
            .map_err(|_| JwtError::Malformed("empty secret"))?;
        mac.update(signing_input.as_bytes());
        let expected = mac.finalize().into_bytes();
        if signature.len() != expected.len()
           || !bool::from(signature.ct_eq(expected.as_slice())) {
            return Err(JwtError::BadSignature);
        }

        let claims: serde_json::Value = serde_json::from_slice(&payload_bytes)
            .map_err(|e| JwtError::Json(format!("payload: {e}")))?;
        let std: StandardClaims = serde_json::from_slice(&payload_bytes)
            .map_err(|e| JwtError::Json(format!("standard-claims: {e}")))?;

        let now_secs = now.duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs()).unwrap_or(0);

        if let Some(exp) = std.exp {
            if now_secs > exp.saturating_add(self.skew_secs) {
                return Err(JwtError::Expired { exp, now: now_secs });
            }
        }
        if let Some(nbf) = std.nbf {
            if now_secs.saturating_add(self.skew_secs) < nbf {
                return Err(JwtError::NotYetValid { nbf, now: now_secs });
            }
        }
        if let Some(want) = &self.expected_iss {
            match &std.iss {
                Some(got) if got == want => {}
                _ => return Err(JwtError::BadSignature),
            }
        }
        if let Some(want) = &self.expected_aud {
            if !matches_audience(&std.aud, want) {
                return Err(JwtError::BadSignature);
            }
        }

        Ok(Verified { header, claims, std })
    }

    pub fn verify(&self, token: &str) -> JwtResult<Verified> {
        self.verify_at(token, SystemTime::now())
    }
}

// ---------------------------------------------------------------------------
// ADR 0328 — RS256 verifier (RSA-SHA256 PKCS#1 v1.5).

/// RS256 verifier. Holds an `rsa::RsaPublicKey` plus the
/// same issuer / audience / skew policy knobs as the HS256
/// verifier.
#[derive(Clone, Debug)]
pub struct JwtRs256Verifier {
    key: RsaPublicKey,
    expected_iss: Option<String>,
    expected_aud: Option<String>,
    skew_secs:    u64,
}

impl JwtRs256Verifier {
    pub fn new(key: RsaPublicKey) -> Self {
        Self { key, expected_iss: None, expected_aud: None, skew_secs: 30 }
    }

    /// Load the verifier from a PEM-encoded SPKI
    /// `-----BEGIN PUBLIC KEY-----` block (the format every
    /// mainstream IdP publishes).
    pub fn from_public_key_pem(pem: &str) -> JwtResult<Self> {
        use rsa::pkcs8::DecodePublicKey;
        let key = RsaPublicKey::from_public_key_pem(pem)
            .map_err(|e| JwtError::KeyParse(format!("SPKI PEM: {e}")))?;
        Ok(Self::new(key))
    }

    pub fn with_issuer  (mut self, iss: impl Into<String>) -> Self {
        self.expected_iss = Some(iss.into()); self
    }
    pub fn with_audience(mut self, aud: impl Into<String>) -> Self {
        self.expected_aud = Some(aud.into()); self
    }
    pub fn with_clock_skew_secs(mut self, secs: u64) -> Self {
        self.skew_secs = secs; self
    }

    pub fn verify(&self, token: &str) -> JwtResult<Verified> {
        self.verify_at(token, SystemTime::now())
    }

    pub fn verify_at(&self, token: &str, now: SystemTime) -> JwtResult<Verified> {
        let mut parts = token.split('.');
        let h_b64 = parts.next().ok_or(JwtError::Malformed("missing header"))?;
        let p_b64 = parts.next().ok_or(JwtError::Malformed("missing payload"))?;
        let s_b64 = parts.next().ok_or(JwtError::Malformed("missing signature"))?;
        if parts.next().is_some() {
            return Err(JwtError::Malformed("extra segments after signature"));
        }

        let header_bytes  = decode_b64(h_b64)?;
        let payload_bytes = decode_b64(p_b64)?;
        let signature     = decode_b64(s_b64)?;

        let header: JwtHeader = serde_json::from_slice(&header_bytes)
            .map_err(|e| JwtError::Json(format!("header: {e}")))?;
        if header.alg != "RS256" {
            return Err(JwtError::UnsupportedAlg(header.alg.clone()));
        }

        // Verify RSA-SHA256 PKCS#1 v1.5 signature over the
        // ASCII bytes of "header.payload".
        let signing_input = format!("{h_b64}.{p_b64}");
        let sig = RsaSignature::try_from(signature.as_slice())
            .map_err(|_| JwtError::BadSignature)?;
        let verifying = RsaVerifyingKey::<Sha256>::new(self.key.clone());
        verifying.verify(signing_input.as_bytes(), &sig)
            .map_err(|_| JwtError::BadSignature)?;

        let claims: serde_json::Value = serde_json::from_slice(&payload_bytes)
            .map_err(|e| JwtError::Json(format!("payload: {e}")))?;
        let std: StandardClaims = serde_json::from_slice(&payload_bytes)
            .map_err(|e| JwtError::Json(format!("standard-claims: {e}")))?;

        let now_secs = now.duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs()).unwrap_or(0);
        if let Some(exp) = std.exp {
            if now_secs > exp.saturating_add(self.skew_secs) {
                return Err(JwtError::Expired { exp, now: now_secs });
            }
        }
        if let Some(nbf) = std.nbf {
            if now_secs.saturating_add(self.skew_secs) < nbf {
                return Err(JwtError::NotYetValid { nbf, now: now_secs });
            }
        }
        if let Some(want) = &self.expected_iss {
            match &std.iss {
                Some(got) if got == want => {}
                _ => return Err(JwtError::BadSignature),
            }
        }
        if let Some(want) = &self.expected_aud {
            if !matches_audience(&std.aud, want) {
                return Err(JwtError::BadSignature);
            }
        }
        Ok(Verified { header, claims, std })
    }
}

// ---------------------------------------------------------------------------
// ADR 0341 — ES256 verifier (ECDSA P-256 SHA-256).

use p256::ecdsa::{
    signature::Verifier as _P256Verifier,
    Signature as EcdsaSignature,
    VerifyingKey as EcdsaVerifyingKey,
};

/// ES256 verifier. Holds a `p256::ecdsa::VerifyingKey`
/// plus the same issuer / audience / skew knobs as the
/// HS256 and RS256 verifiers.
#[derive(Clone, Debug)]
pub struct JwtEs256Verifier {
    key: EcdsaVerifyingKey,
    expected_iss: Option<String>,
    expected_aud: Option<String>,
    skew_secs:    u64,
}

impl JwtEs256Verifier {
    pub fn new(key: EcdsaVerifyingKey) -> Self {
        Self { key, expected_iss: None, expected_aud: None, skew_secs: 30 }
    }

    /// Load from a PEM-encoded SPKI
    /// `-----BEGIN PUBLIC KEY-----` block (the shape
    /// every IdP that supports ES256 publishes).
    pub fn from_public_key_pem(pem: &str) -> JwtResult<Self> {
        use p256::pkcs8::DecodePublicKey;
        let key = EcdsaVerifyingKey::from_public_key_pem(pem)
            .map_err(|e| JwtError::KeyParse(format!("SPKI PEM: {e}")))?;
        Ok(Self::new(key))
    }

    pub fn with_issuer(mut self, iss: impl Into<String>) -> Self {
        self.expected_iss = Some(iss.into()); self
    }
    pub fn with_audience(mut self, aud: impl Into<String>) -> Self {
        self.expected_aud = Some(aud.into()); self
    }
    pub fn with_clock_skew_secs(mut self, secs: u64) -> Self {
        self.skew_secs = secs; self
    }

    pub fn verify(&self, token: &str) -> JwtResult<Verified> {
        self.verify_at(token, SystemTime::now())
    }

    pub fn verify_at(&self, token: &str, now: SystemTime) -> JwtResult<Verified> {
        let mut parts = token.split('.');
        let h_b64 = parts.next().ok_or(JwtError::Malformed("missing header"))?;
        let p_b64 = parts.next().ok_or(JwtError::Malformed("missing payload"))?;
        let s_b64 = parts.next().ok_or(JwtError::Malformed("missing signature"))?;
        if parts.next().is_some() {
            return Err(JwtError::Malformed("extra segments after signature"));
        }

        let header_bytes  = decode_b64(h_b64)?;
        let payload_bytes = decode_b64(p_b64)?;
        let signature     = decode_b64(s_b64)?;

        let header: JwtHeader = serde_json::from_slice(&header_bytes)
            .map_err(|e| JwtError::Json(format!("header: {e}")))?;
        if header.alg != "ES256" {
            return Err(JwtError::UnsupportedAlg(header.alg.clone()));
        }

        // ES256 JWS signatures are the raw 64-byte r||s
        // IEEE-P1363 concatenation (NOT DER-wrapped). The
        // `p256` crate's `Signature::from_slice` handles
        // this shape directly.
        let sig = EcdsaSignature::from_slice(&signature)
            .map_err(|_| JwtError::BadSignature)?;
        let signing_input = format!("{h_b64}.{p_b64}");
        self.key.verify(signing_input.as_bytes(), &sig)
            .map_err(|_| JwtError::BadSignature)?;

        let claims: serde_json::Value = serde_json::from_slice(&payload_bytes)
            .map_err(|e| JwtError::Json(format!("payload: {e}")))?;
        let std: StandardClaims = serde_json::from_slice(&payload_bytes)
            .map_err(|e| JwtError::Json(format!("standard-claims: {e}")))?;

        let now_secs = now.duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs()).unwrap_or(0);
        if let Some(exp) = std.exp {
            if now_secs > exp.saturating_add(self.skew_secs) {
                return Err(JwtError::Expired { exp, now: now_secs });
            }
        }
        if let Some(nbf) = std.nbf {
            if now_secs.saturating_add(self.skew_secs) < nbf {
                return Err(JwtError::NotYetValid { nbf, now: now_secs });
            }
        }
        if let Some(want) = &self.expected_iss {
            match &std.iss {
                Some(got) if got == want => {}
                _ => return Err(JwtError::BadSignature),
            }
        }
        if let Some(want) = &self.expected_aud {
            if !matches_audience(&std.aud, want) {
                return Err(JwtError::BadSignature);
            }
        }
        Ok(Verified { header, claims, std })
    }
}

// ---------------------------------------------------------------------------

fn decode_b64(s: &str) -> JwtResult<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s).map_err(|e| JwtError::Base64(format!("{e}")))
}

fn matches_audience(aud: &Option<serde_json::Value>, want: &str) -> bool {
    match aud {
        Some(serde_json::Value::String(s)) => s == want,
        Some(serde_json::Value::Array(xs)) => xs.iter().any(|v|
            v.as_str().map(|s| s == want).unwrap_or(false)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Build a test token with the given claims JSON.
    fn mint(secret: &[u8], claims: serde_json::Value) -> String {
        let header = serde_json::json!({ "alg": "HS256", "typ": "JWT" });
        let h = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap());
        let p = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let signing = format!("{h}.{p}");
        let mut mac = <HmacSha256 as Mac>::new_from_slice(secret).unwrap();
        mac.update(signing.as_bytes());
        let s = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        format!("{signing}.{s}")
    }

    fn now_at(secs_from_epoch: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs_from_epoch)
    }

    #[test]
    fn valid_hs256_token_verifies_and_exposes_claims() {
        let secret = b"shared-secret";
        let token = mint(secret, serde_json::json!({
            "iss": "https://idp.example",
            "sub": "alice",
            "exp": 10_000,
        }));
        let v = JwtHs256Verifier::new(secret)
            .verify_at(&token, now_at(9_000)).unwrap();
        assert_eq!(v.header.alg, "HS256");
        assert_eq!(v.std.sub.as_deref(), Some("alice"));
        assert_eq!(v.claims["iss"], "https://idp.example");
    }

    #[test]
    fn tampered_payload_fails_signature_check() {
        let secret = b"s";
        let token = mint(secret, serde_json::json!({ "sub": "alice", "exp": 10_000 }));
        // Flip one base64 char in the payload segment.
        let mut parts: Vec<&str> = token.splitn(3, '.').collect();
        let mut payload = parts[1].to_string();
        let b = payload.pop().unwrap();
        payload.push(if b == 'A' { 'B' } else { 'A' });
        parts[1] = &payload;
        let tampered = format!("{}.{}.{}", parts[0], parts[1], parts[2]);
        let err = JwtHs256Verifier::new(secret).verify_at(&tampered, now_at(0)).unwrap_err();
        assert_eq!(err, JwtError::BadSignature);
    }

    #[test]
    fn wrong_secret_fails_signature_check() {
        let token = mint(b"real", serde_json::json!({ "sub": "a", "exp": 10_000 }));
        let err = JwtHs256Verifier::new(b"bogus").verify_at(&token, now_at(0)).unwrap_err();
        assert_eq!(err, JwtError::BadSignature);
    }

    #[test]
    fn expired_token_is_rejected_past_skew_window() {
        let secret = b"s";
        let token = mint(secret, serde_json::json!({ "sub": "a", "exp": 100 }));
        let v = JwtHs256Verifier::new(secret).with_clock_skew_secs(30);
        // Within skew → ok.
        v.verify_at(&token, now_at(120)).unwrap();
        // Past skew → Expired.
        let err = v.verify_at(&token, now_at(200)).unwrap_err();
        assert!(matches!(err, JwtError::Expired { .. }), "{err:?}");
    }

    #[test]
    fn not_yet_valid_token_is_rejected() {
        let secret = b"s";
        let token = mint(secret, serde_json::json!({ "sub": "a", "nbf": 1000, "exp": 10_000 }));
        let v = JwtHs256Verifier::new(secret).with_clock_skew_secs(5);
        let err = v.verify_at(&token, now_at(500)).unwrap_err();
        assert!(matches!(err, JwtError::NotYetValid { .. }), "{err:?}");
        // Inside the window → ok.
        v.verify_at(&token, now_at(1500)).unwrap();
    }

    #[test]
    fn unsupported_algorithm_in_header_is_rejected() {
        // Hand-craft a token with alg=none.
        let secret = b"s";
        let header = serde_json::json!({ "alg": "none", "typ": "JWT" });
        let payload = serde_json::json!({ "sub": "a", "exp": 10_000 });
        let h = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap());
        let p = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("{h}.{p}.");
        let err = JwtHs256Verifier::new(secret).verify_at(&token, now_at(0)).unwrap_err();
        assert!(matches!(err, JwtError::UnsupportedAlg(a) if a == "none"));
    }

    #[test]
    fn malformed_token_is_rejected_at_each_segment() {
        let v = JwtHs256Verifier::new(b"s");
        assert!(matches!(v.verify_at("", now_at(0)), Err(JwtError::Malformed(_))));
        assert!(matches!(v.verify_at("only-one-dot.yeah", now_at(0)),
            Err(JwtError::Malformed(_))));
        assert!(matches!(v.verify_at("a.b.c.d", now_at(0)),
            Err(JwtError::Malformed(_))));
    }

    #[test]
    fn issuer_and_audience_are_enforced_when_expected() {
        let secret = b"s";
        let token = mint(secret, serde_json::json!({
            "iss": "https://idp", "aud": "bradian", "sub": "a", "exp": 10_000,
        }));
        let v = JwtHs256Verifier::new(secret)
            .with_issuer("https://idp")
            .with_audience("bradian");
        v.verify_at(&token, now_at(0)).unwrap();
        let bad_iss = JwtHs256Verifier::new(secret)
            .with_issuer("https://other");
        assert_eq!(bad_iss.verify_at(&token, now_at(0)).unwrap_err(), JwtError::BadSignature);
        let bad_aud = JwtHs256Verifier::new(secret)
            .with_audience("elsewhere");
        assert_eq!(bad_aud.verify_at(&token, now_at(0)).unwrap_err(), JwtError::BadSignature);
    }

    #[test]
    fn audience_array_matches_any_entry() {
        let secret = b"s";
        let token = mint(secret, serde_json::json!({
            "aud": ["a", "bradian", "c"], "sub": "u", "exp": 10_000,
        }));
        let v = JwtHs256Verifier::new(secret).with_audience("bradian");
        v.verify_at(&token, now_at(0)).unwrap();
        let bad = JwtHs256Verifier::new(secret).with_audience("d");
        assert_eq!(bad.verify_at(&token, now_at(0)).unwrap_err(), JwtError::BadSignature);
    }

    // ----------------------------------------------------------
    // ADR 0328 — RS256 tests. Fixture keypair generated once
    // per test from a deterministic seed so runs are
    // reproducible (ChaCha20Rng with fixed seed).

    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;
    use rsa::pkcs1v15::SigningKey as RsaSigningKey;
    use rsa::pkcs8::EncodePublicKey as _;
    use rsa::signature::{RandomizedSigner, SignatureEncoding};
    use rsa::RsaPrivateKey;

    fn fixture_rsa_keypair() -> (RsaPrivateKey, rsa::RsaPublicKey) {
        // 2048 bits — ensures the signature is 256 bytes
        // (standard RS256 fixture size).
        let mut rng = ChaCha20Rng::seed_from_u64(0x6272_6164_6961_6e00); // "bradia\0"
        let priv_key = RsaPrivateKey::new(&mut rng, 2048)
            .expect("fixture 2048-bit RSA key");
        let pub_key = priv_key.to_public_key();
        (priv_key, pub_key)
    }

    fn mint_rs256(priv_key: &RsaPrivateKey, claims: serde_json::Value) -> String {
        let header = serde_json::json!({ "alg": "RS256", "typ": "JWT" });
        let h = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap());
        let p = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let signing = format!("{h}.{p}");
        let signing_key = RsaSigningKey::<Sha256>::new(priv_key.clone());
        let mut rng = ChaCha20Rng::seed_from_u64(0x1111_2222_3333_4444);
        let sig = signing_key.sign_with_rng(&mut rng, signing.as_bytes());
        let s = URL_SAFE_NO_PAD.encode(sig.to_bytes());
        format!("{signing}.{s}")
    }

    #[test]
    fn rs256_round_trip_verifies_a_fixture_signed_token() {
        let (priv_key, pub_key) = fixture_rsa_keypair();
        let token = mint_rs256(&priv_key, serde_json::json!({
            "iss": "https://idp.example",
            "sub": "alice",
            "exp": 10_000_u64,
        }));
        let v = JwtRs256Verifier::new(pub_key);
        let verified = v.verify_at(&token, now_at(5_000)).unwrap();
        assert_eq!(verified.header.alg, "RS256");
        assert_eq!(verified.std.sub.as_deref(), Some("alice"));
    }

    #[test]
    fn rs256_tampered_payload_fails_signature_check() {
        let (priv_key, pub_key) = fixture_rsa_keypair();
        let token = mint_rs256(&priv_key, serde_json::json!({
            "sub": "a", "exp": 10_000_u64,
        }));
        let parts: Vec<&str> = token.splitn(3, '.').collect();
        // Flip one bit in the payload.
        let mut payload = parts[1].to_string();
        let b = payload.pop().unwrap();
        payload.push(if b == 'A' { 'B' } else { 'A' });
        let tampered = format!("{}.{}.{}", parts[0], payload, parts[2]);
        let err = JwtRs256Verifier::new(pub_key).verify_at(&tampered, now_at(0)).unwrap_err();
        assert_eq!(err, JwtError::BadSignature);
    }

    #[test]
    fn rs256_wrong_key_fails_signature_check() {
        let (priv_a, _) = fixture_rsa_keypair();
        // Generate an unrelated key with a different seed.
        let mut rng = ChaCha20Rng::seed_from_u64(0xdeadbeef);
        let priv_b = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let pub_b  = priv_b.to_public_key();
        let token = mint_rs256(&priv_a, serde_json::json!({
            "sub": "a", "exp": 10_000_u64,
        }));
        let err = JwtRs256Verifier::new(pub_b).verify_at(&token, now_at(0)).unwrap_err();
        assert_eq!(err, JwtError::BadSignature);
    }

    #[test]
    fn rs256_wrong_alg_in_header_rejected() {
        let (priv_key, pub_key) = fixture_rsa_keypair();
        let token = mint_rs256(&priv_key, serde_json::json!({ "sub": "a", "exp": 10_000_u64 }));
        // Replace the header's alg with HS256 (still base64-encoded; the
        // body changes, so the signature will not be valid but we
        // check that the alg check fires first).
        let bad_header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
        let parts: Vec<&str> = token.splitn(3, '.').collect();
        let twisted = format!("{}.{}.{}", bad_header, parts[1], parts[2]);
        let err = JwtRs256Verifier::new(pub_key).verify_at(&twisted, now_at(0)).unwrap_err();
        assert!(matches!(err, JwtError::UnsupportedAlg(a) if a == "HS256"));
    }

    #[test]
    fn rs256_expired_rejected_past_skew() {
        let (priv_key, pub_key) = fixture_rsa_keypair();
        let token = mint_rs256(&priv_key, serde_json::json!({ "sub": "a", "exp": 100_u64 }));
        let v = JwtRs256Verifier::new(pub_key).with_clock_skew_secs(30);
        v.verify_at(&token, now_at(120)).unwrap();      // within skew
        let err = v.verify_at(&token, now_at(200)).unwrap_err();
        assert!(matches!(err, JwtError::Expired { .. }), "{err:?}");
    }

    #[test]
    fn rs256_iss_aud_enforced_when_expected() {
        let (priv_key, pub_key) = fixture_rsa_keypair();
        let token = mint_rs256(&priv_key, serde_json::json!({
            "iss": "https://idp", "aud": "bradian",
            "sub": "a", "exp": 10_000_u64,
        }));
        let ok = JwtRs256Verifier::new(pub_key.clone())
            .with_issuer("https://idp").with_audience("bradian");
        ok.verify_at(&token, now_at(0)).unwrap();
        let bad_iss = JwtRs256Verifier::new(pub_key.clone())
            .with_issuer("https://other");
        assert_eq!(bad_iss.verify_at(&token, now_at(0)).unwrap_err(), JwtError::BadSignature);
        let bad_aud = JwtRs256Verifier::new(pub_key).with_audience("elsewhere");
        assert_eq!(bad_aud.verify_at(&token, now_at(0)).unwrap_err(), JwtError::BadSignature);
    }

    #[test]
    fn rs256_verifier_loads_from_spki_pem() {
        let (_, pub_key) = fixture_rsa_keypair();
        let pem = pub_key.to_public_key_pem(rsa::pkcs8::LineEnding::LF).unwrap();
        assert!(pem.contains("BEGIN PUBLIC KEY"));
        // Construct a verifier from the PEM string.
        let v = JwtRs256Verifier::from_public_key_pem(&pem).unwrap();
        // And a malformed PEM is rejected.
        let bad = JwtRs256Verifier::from_public_key_pem("not a pem");
        assert!(matches!(bad, Err(JwtError::KeyParse(_))));
        let _ = v;   // silence unused
    }

    // ----------------------------------------------------------
    // ADR 0341 — ES256 tests.

    use p256::ecdsa::{signature::Signer as _, SigningKey as EcdsaSigningKey};
    #[allow(unused_imports)]
    use p256::pkcs8::EncodePublicKey as _;

    fn fixture_ec_keypair(seed: u64) -> (EcdsaSigningKey, EcdsaVerifyingKey) {
        let mut rng = ChaCha20Rng::seed_from_u64(seed);
        let sk = EcdsaSigningKey::random(&mut rng);
        let vk = *sk.verifying_key();
        (sk, vk)
    }

    fn mint_es256(sk: &EcdsaSigningKey, claims: serde_json::Value) -> String {
        let header = serde_json::json!({ "alg": "ES256", "typ": "JWT" });
        let h = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap());
        let p = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let signing = format!("{h}.{p}");
        let sig: EcdsaSignature = sk.sign(signing.as_bytes());
        // JWS ES256 is raw r||s (IEEE P1363), 64 bytes.
        format!("{signing}.{}", URL_SAFE_NO_PAD.encode(sig.to_bytes()))
    }

    #[test]
    fn es256_round_trip_verifies_a_fixture_signed_token() {
        let (sk, vk) = fixture_ec_keypair(1);
        let token = mint_es256(&sk, serde_json::json!({
            "iss": "https://idp.example",
            "sub": "alice",
            "exp": 10_000_u64,
        }));
        let v = JwtEs256Verifier::new(vk)
            .verify_at(&token, now_at(5_000)).unwrap();
        assert_eq!(v.header.alg, "ES256");
        assert_eq!(v.std.sub.as_deref(), Some("alice"));
    }

    #[test]
    fn es256_tampered_payload_fails_signature_check() {
        let (sk, vk) = fixture_ec_keypair(2);
        let token = mint_es256(&sk, serde_json::json!({
            "sub": "a", "exp": 10_000_u64,
        }));
        let parts: Vec<&str> = token.splitn(3, '.').collect();
        let mut payload = parts[1].to_string();
        let b = payload.pop().unwrap();
        payload.push(if b == 'A' { 'B' } else { 'A' });
        let tampered = format!("{}.{}.{}", parts[0], payload, parts[2]);
        let err = JwtEs256Verifier::new(vk).verify_at(&tampered, now_at(0)).unwrap_err();
        assert_eq!(err, JwtError::BadSignature);
    }

    #[test]
    fn es256_wrong_key_fails_signature_check() {
        let (sk_a, _)    = fixture_ec_keypair(3);
        let (_, vk_b)    = fixture_ec_keypair(4);
        let token = mint_es256(&sk_a, serde_json::json!({
            "sub": "a", "exp": 10_000_u64,
        }));
        let err = JwtEs256Verifier::new(vk_b).verify_at(&token, now_at(0)).unwrap_err();
        assert_eq!(err, JwtError::BadSignature);
    }

    #[test]
    fn es256_wrong_alg_in_header_is_rejected_before_crypto() {
        let (sk, vk) = fixture_ec_keypair(5);
        let token = mint_es256(&sk, serde_json::json!({ "sub": "a", "exp": 10_000_u64 }));
        let bad_header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let parts: Vec<&str> = token.splitn(3, '.').collect();
        let twisted = format!("{}.{}.{}", bad_header, parts[1], parts[2]);
        let err = JwtEs256Verifier::new(vk).verify_at(&twisted, now_at(0)).unwrap_err();
        assert!(matches!(err, JwtError::UnsupportedAlg(a) if a == "RS256"));
    }

    #[test]
    fn es256_verifier_loads_from_spki_pem() {
        let (_, vk) = fixture_ec_keypair(6);
        let pem = vk.to_public_key_pem(p256::pkcs8::LineEnding::LF).unwrap();
        assert!(pem.contains("BEGIN PUBLIC KEY"));
        JwtEs256Verifier::from_public_key_pem(&pem).unwrap();
        assert!(matches!(
            JwtEs256Verifier::from_public_key_pem("garbage"),
            Err(JwtError::KeyParse(_))));
    }

    #[test]
    fn es256_iss_and_aud_enforced() {
        let (sk, vk) = fixture_ec_keypair(7);
        let token = mint_es256(&sk, serde_json::json!({
            "iss": "https://idp", "aud": "bradian",
            "sub": "a", "exp": 10_000_u64,
        }));
        JwtEs256Verifier::new(vk)
            .with_issuer("https://idp")
            .with_audience("bradian")
            .verify_at(&token, now_at(0)).unwrap();
        let (_, vk2) = fixture_ec_keypair(7);
        let bad = JwtEs256Verifier::new(vk2).with_issuer("https://other");
        assert_eq!(bad.verify_at(&token, now_at(0)).unwrap_err(),
            JwtError::BadSignature);
    }
}
