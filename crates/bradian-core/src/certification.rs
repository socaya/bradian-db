//! ADR 0319 — External reproducibility + production
//! certification **skeleton**.
//!
//! Shape validation + JSON round-trip. No reproducible-
//! build check, no supply-chain audit, no signature
//! algorithm. ADR 0326 CI infra will later integrate this
//! with build artefacts.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertManifest {
    pub commit_sha:            String,          // 40 hex chars
    pub cargo_lock_hash:       String,          // 64 hex chars (BLAKE3)
    pub rustc_version:         String,
    pub verified_tests:        Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature_placeholder: Option<String>,
}

#[derive(Debug, Error)]
pub enum CertError {
    #[error("commit_sha must be 40 hex chars; got {0} chars")]
    CommitShape(usize),
    #[error("cargo_lock_hash must be 64 hex chars; got {0} chars")]
    LockShape(usize),
    #[error("rustc_version must not be empty")]
    EmptyRustc,
    #[error("verified_tests must not be empty")]
    EmptyTests,
}
pub type CertResult<T> = Result<T, CertError>;

impl CertManifest {
    pub fn validate_shape(&self) -> CertResult<()> {
        if self.commit_sha.len() != 40 || !self.commit_sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CertError::CommitShape(self.commit_sha.len()));
        }
        if self.cargo_lock_hash.len() != 64
           || !self.cargo_lock_hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CertError::LockShape(self.cargo_lock_hash.len()));
        }
        if self.rustc_version.is_empty() { return Err(CertError::EmptyRustc); }
        if self.verified_tests.is_empty() { return Err(CertError::EmptyTests); }
        Ok(())
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("cert manifest serialize")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> CertManifest {
        CertManifest {
            commit_sha:            "a".repeat(40),
            cargo_lock_hash:       "0".repeat(64),
            rustc_version:         "rustc 1.92.0".into(),
            verified_tests:        vec!["cargo test --workspace --locked".into()],
            signature_placeholder: None,
        }
    }

    #[test]
    fn valid_manifest_passes_shape_check_and_round_trips() {
        let m = valid();
        m.validate_shape().unwrap();
        let j = m.to_json();
        let back: CertManifest = serde_json::from_str(&j).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn malformed_commit_sha_and_lock_hash_are_rejected() {
        let mut m = valid();
        m.commit_sha = "short".into();
        assert!(matches!(m.validate_shape(), Err(CertError::CommitShape(_))));
        m = valid();
        m.cargo_lock_hash = "z".repeat(64);   // right length, wrong alphabet
        assert!(matches!(m.validate_shape(), Err(CertError::LockShape(_))));
    }
}
