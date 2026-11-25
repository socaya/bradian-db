//! Authentication, authorization, audit, cache keying.
//!
//! Requirements: SEC-001/002/003 (v1 minimum: bearer tokens, OU-scoped policy, audit log).
#![deny(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod audit;
pub mod authz;
pub mod encryption;
/// ADR 0323 — enterprise OIDC JWT verification.
pub mod enterprise_identity;
pub mod principal;
pub mod token;

pub use audit::{AuditEntry, AuditLog};
pub use authz::{cache_key, policy_predicate, PolicyContext};
pub use encryption::{seal, unseal, DataKey, KeyId, KeyRing, SealedPayload};
pub use enterprise_identity::{JwtError, JwtHeader, JwtHs256Verifier, JwtResult, StandardClaims, Verified};
pub use principal::{Principal, Role};
pub use token::TokenStore;
