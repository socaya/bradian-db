//! ADR 0319 — External reproducibility + production
//! certification.
//!
//! A certification manifest binds a build artefact to its
//! **reproducible inputs**: the git commit SHA, the BLAKE3
//! hash of the lock file that pinned every dependency, the
//! rustc version used, and a declared set of test runs
//! that were observed green. On reload the manifest can
//! verify itself against the current `Cargo.lock` on disk
//! and refuse a session whose lock-file bytes drifted.
//!
//! This slice does the real reproducibility check for the
//! lock-file axis. It does NOT yet sign the manifest nor
//! verify signatures — the `signature` field is reserved
//! for a downstream ADR that chooses an algorithm and key
//! material (ADR 0326 CI infra is its likely home).

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Hex-encoded BLAKE3-256 of the pinned `Cargo.lock` bytes.
const LOCK_HASH_HEX_LEN: usize = 64;
/// Hex-encoded git SHA-1.
const COMMIT_HEX_LEN:    usize = 40;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertManifest {
    pub commit_sha:      String,
    /// BLAKE3-256 of the pinned `Cargo.lock` bytes, hex.
    pub cargo_lock_hash: String,
    pub rustc_version:   String,
    pub verified_tests:  Vec<String>,
    /// Reserved. A future ADR fills this with the result
    /// of a chosen signature algorithm over the remaining
    /// fields; `None` means "unsigned manifest" and
    /// `verify_against` still works, it just doesn't gate
    /// on provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature:       Option<String>,
}

#[derive(Debug, Error)]
pub enum CertError {
    #[error("commit_sha must be {COMMIT_HEX_LEN} hex chars; got {0} chars")]
    CommitShape(usize),
    #[error("cargo_lock_hash must be {LOCK_HASH_HEX_LEN} hex chars; got {0} chars")]
    LockShape(usize),
    #[error("rustc_version must not be empty")]
    EmptyRustc,
    #[error("verified_tests must not be empty")]
    EmptyTests,
    #[error("Cargo.lock drift detected: manifest={manifest}, actual={actual}")]
    LockDrift { manifest: String, actual: String },
    #[error("io reading Cargo.lock: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type CertResult<T> = Result<T, CertError>;

impl CertManifest {
    /// Build a manifest by hashing the given `Cargo.lock`
    /// on disk. The caller supplies the commit SHA, rustc
    /// version string and the test runs it has declared
    /// green — these are build-time inputs that this
    /// function cannot observe itself.
    pub fn from_build(
        commit_sha:      impl Into<String>,
        rustc_version:   impl Into<String>,
        verified_tests:  Vec<String>,
        cargo_lock_path: &Path,
    ) -> CertResult<Self> {
        let cargo_lock_hash = hash_file_blake3_hex(cargo_lock_path)?;
        let m = Self {
            commit_sha:      commit_sha.into(),
            cargo_lock_hash,
            rustc_version:   rustc_version.into(),
            verified_tests,
            signature:       None,
        };
        m.validate_shape()?;
        Ok(m)
    }

    pub fn validate_shape(&self) -> CertResult<()> {
        if self.commit_sha.len() != COMMIT_HEX_LEN
           || !self.commit_sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CertError::CommitShape(self.commit_sha.len()));
        }
        if self.cargo_lock_hash.len() != LOCK_HASH_HEX_LEN
           || !self.cargo_lock_hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CertError::LockShape(self.cargo_lock_hash.len()));
        }
        if self.rustc_version.is_empty() { return Err(CertError::EmptyRustc); }
        if self.verified_tests.is_empty() { return Err(CertError::EmptyTests); }
        Ok(())
    }

    /// Re-hash the current `Cargo.lock` on disk and refuse
    /// if the bytes drifted from what the manifest recorded.
    /// This is the real reproducibility gate.
    pub fn verify_against(&self, cargo_lock_path: &Path) -> CertResult<()> {
        self.validate_shape()?;
        let actual = hash_file_blake3_hex(cargo_lock_path)?;
        if !ct_eq(actual.as_bytes(), self.cargo_lock_hash.as_bytes()) {
            return Err(CertError::LockDrift {
                manifest: self.cargo_lock_hash.clone(), actual,
            });
        }
        Ok(())
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("cert manifest serialize")
    }

    pub fn from_json(bytes: &[u8]) -> CertResult<Self> {
        let m: Self = serde_json::from_slice(bytes)?;
        m.validate_shape()?;
        Ok(m)
    }
}

/// Stream-hash a file into hex-encoded BLAKE3-256. Reads in
/// 64 KiB chunks so arbitrarily large lock files don't
/// require holding the whole thing in memory.
fn hash_file_blake3_hex(path: &Path) -> CertResult<String> {
    let f = File::open(path)?;
    let mut reader = BufReader::new(f);
    let mut hasher = blake3::Hasher::new();
    let mut buf    = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize().as_bytes()))
}

/// Constant-time byte-slice equality. The hash itself is
/// public, but the comparison path runs before every
/// reload so we do the right thing.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}

/// Minimal hex encoder (we intentionally do not add a new
/// workspace dep for one function).
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            out.push(HEX[(*b >> 4) as usize] as char);
            out.push(HEX[(*b & 0x0f) as usize] as char);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(contents: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Cargo.lock");
        let mut f = File::create(&path).unwrap();
        f.write_all(contents).unwrap();
        f.sync_all().unwrap();
        (dir, path)
    }

    #[test]
    fn hash_matches_blake3_reference_for_known_input() {
        let (_dir, path) = write_tmp(b"hello bradian");
        let hex = hash_file_blake3_hex(&path).unwrap();
        // Compare against a direct in-memory BLAKE3 hash so
        // the streaming path is proven equivalent.
        let direct = blake3::hash(b"hello bradian");
        assert_eq!(hex, super::hex::encode(direct.as_bytes()));
        assert_eq!(hex.len(), LOCK_HASH_HEX_LEN);
    }

    #[test]
    fn from_build_populates_hash_and_shape_validates() {
        let (_dir, path) = write_tmp(b"[workspace]\nmembers = []\n");
        let m = CertManifest::from_build(
            "a".repeat(COMMIT_HEX_LEN),
            "rustc 1.92.0 (ded5c06cf 2025-12-08)",
            vec!["cargo test --workspace --locked".into()],
            &path,
        ).unwrap();
        m.validate_shape().unwrap();
        assert_eq!(m.cargo_lock_hash.len(), LOCK_HASH_HEX_LEN);
    }

    #[test]
    fn verify_against_matches_the_exact_file_bytes() {
        let (_dir, path) = write_tmp(b"same bytes");
        let m = CertManifest::from_build(
            "b".repeat(COMMIT_HEX_LEN), "rustc 1.0",
            vec!["t".into()], &path).unwrap();
        m.verify_against(&path).unwrap();
    }

    #[test]
    fn verify_against_detects_single_byte_drift() {
        let (dir, path) = write_tmp(b"original");
        let m = CertManifest::from_build(
            "c".repeat(COMMIT_HEX_LEN), "rustc 1.0",
            vec!["t".into()], &path).unwrap();
        // Replace the file with drifted contents.
        let _ = dir;  // keep tempdir alive
        std::fs::write(&path, b"original!").unwrap();
        let err = m.verify_against(&path).unwrap_err();
        match err {
            CertError::LockDrift { manifest, actual } => {
                assert_eq!(manifest, m.cargo_lock_hash);
                assert_ne!(actual,   m.cargo_lock_hash);
            }
            other => panic!("expected LockDrift, got {other:?}"),
        }
    }

    #[test]
    fn json_round_trip_preserves_every_field() {
        let (_dir, path) = write_tmp(b"json rt");
        let mut m = CertManifest::from_build(
            "d".repeat(COMMIT_HEX_LEN),
            "rustc 1.92.0",
            vec!["cargo test".into(), "cargo clippy".into()],
            &path,
        ).unwrap();
        m.signature = Some("ed25519:…".into());
        let bytes = m.to_json().into_bytes();
        let back = CertManifest::from_json(&bytes).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn malformed_shape_rejected_at_load() {
        let bad = serde_json::json!({
            "commit_sha":      "short",
            "cargo_lock_hash": "0".repeat(LOCK_HASH_HEX_LEN),
            "rustc_version":   "rustc 1.0",
            "verified_tests":  ["t"],
        });
        let err = CertManifest::from_json(bad.to_string().as_bytes()).unwrap_err();
        assert!(matches!(err, CertError::CommitShape(_)));
    }

    #[test]
    fn verify_against_missing_file_surfaces_io_error() {
        let (_dir, path) = write_tmp(b"present");
        let m = CertManifest::from_build(
            "e".repeat(COMMIT_HEX_LEN), "rustc 1.0",
            vec!["t".into()], &path).unwrap();
        let err = m.verify_against(std::path::Path::new("/no/such/path")).unwrap_err();
        assert!(matches!(err, CertError::Io(_)), "got {err:?}");
    }
}
