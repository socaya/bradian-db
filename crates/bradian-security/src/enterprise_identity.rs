//! ADR 0323 — Enterprise identity (OIDC JWT verification).
//!
//! Verifies a JSON Web Token (JWS compact format) signed
//! with HS256 against a shared secret. Returns the parsed
//! claims on success, a typed error on failure.
//!
//! HS256 is the conservative shared-secret algorithm;
//! every mainstream IdP (Keycloak, Okta, Azure AD,
//! Google) supports it as a client-credential option.
//! RS256 / ES256 / Ed25519 verification are dedicated
//! follow-ups once an asymmetric-key workflow is agreed
//! with operators.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
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
}
