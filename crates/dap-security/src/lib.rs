//! Authentication, authorization, audit, cache keying.
//!
//! Requirements: SEC-001/002/003 (v1 minimum: bearer tokens, OU-scoped policy, audit log).
#![deny(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod audit;
pub mod authz;
pub mod principal;
pub mod token;

pub use audit::{AuditEntry, AuditLog};
pub use authz::{PolicyContext, policy_predicate, cache_key};
pub use principal::{Principal, Role};
pub use token::TokenStore;
