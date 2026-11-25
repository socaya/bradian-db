//! Authentication, authorization, audit, cache keying.
//!
//! Requirements: SEC-001/002/003 (v1 minimum: bearer tokens, OU-scoped policy, audit log).
#![deny(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod audit;
pub mod authz;
/// ADR 0332 — principal-from-cert mapping for mTLS peers.
pub mod cert_principal;
pub mod encryption;
/// ADR 0323 (HS256) + ADR 0328 (RS256) — enterprise OIDC JWT verification.
pub mod enterprise_identity;
/// ADR 0329 — JWKS fetch + kid-indexed cache.
pub mod jwks;
pub mod principal;
pub mod token;

pub use audit::{AuditEntry, AuditLog};
pub use authz::{cache_key, policy_predicate, PolicyContext};
pub use cert_principal::{
    extract_common_name, principal_from_peer_chain,
    CertPrincipalError, CertPrincipalResult,
};
pub use encryption::{seal, unseal, DataKey, KeyId, KeyRing, SealedPayload};
pub use enterprise_identity::{
    JwtError, JwtHeader, JwtHs256Verifier, JwtResult, JwtRs256Verifier,
    StandardClaims, Verified,
};
pub use jwks::{Jwk, JwkSet, JwksCache, JwksError, JwksResult, rsa_public_key_from_jwk};
pub use principal::{Principal, Role};
pub use token::TokenStore;
