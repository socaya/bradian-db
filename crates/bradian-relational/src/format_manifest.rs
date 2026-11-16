//! ADR 0115 — data-root format version manifest.
//!
//! A tiny `format.manifest` JSON file at the data-root
//! records the format version the directory was last opened
//! under. New opens validate compat.
//!
//! Honest scope: a single integer version plus a human-
//! readable product tag. Operators who upgrade Bradian see
//! a clear error if an older build tries to open a newer
//! root. Downgrade paths are explicitly rejected.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current writer version. Bumped whenever on-disk format
/// changes in a way that older readers cannot handle. Each
/// bump has a matching ADR + verification doc.
pub const CURRENT_FORMAT_VERSION: u32 = 1;
const MANIFEST_FILE: &str = "format.manifest";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FormatManifest {
    pub product: String,
    pub version: u32,
    pub created_at_ms: i64,
    pub last_opened_at_ms: i64,
}

#[derive(Debug, Error)]
pub enum FormatError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("format.manifest version {on_disk} is newer than this build (max {max}); upgrade Bradian before opening")]
    TooNew { on_disk: u32, max: u32 },
    #[error("format.manifest version {on_disk} is older than this build (min {min}); downgrade is not supported")]
    TooOld { on_disk: u32, min: u32 },
    #[error("format.manifest product {on_disk:?} does not match expected {expected:?}")]
    WrongProduct { on_disk: String, expected: String },
}

pub type Result<T> = std::result::Result<T, FormatError>;

/// Open or create the format manifest at `root`. Updates
/// `last_opened_at_ms` on every call. Rejects newer-than-
/// build (operator must upgrade) and older-than-minimum
/// (downgrade refused).
pub fn ensure_manifest(root: &Path) -> Result<FormatManifest> {
    ensure_manifest_with_bounds(root, CURRENT_FORMAT_VERSION, CURRENT_FORMAT_VERSION)
}

/// Test-visible workhorse. Allows exercising min-version
/// rejection without a real downgrade.
pub fn ensure_manifest_with_bounds(
    root: &Path, min: u32, max: u32,
) -> Result<FormatManifest> {
    std::fs::create_dir_all(root)?;
    let path = root.join(MANIFEST_FILE);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let mut manifest = if path.exists() {
        let bytes = std::fs::read(&path)?;
        let m: FormatManifest = serde_json::from_slice(&bytes)?;
        if m.product != "bradian-db" {
            return Err(FormatError::WrongProduct {
                on_disk: m.product,
                expected: "bradian-db".into(),
            });
        }
        if m.version > max {
            return Err(FormatError::TooNew { on_disk: m.version, max });
        }
        if m.version < min {
            return Err(FormatError::TooOld { on_disk: m.version, min });
        }
        m
    } else {
        FormatManifest {
            product: "bradian-db".into(),
            version: CURRENT_FORMAT_VERSION,
            created_at_ms: now,
            last_opened_at_ms: now,
        }
    };
    manifest.last_opened_at_ms = now;
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest)?)?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn fresh_root_creates_manifest_at_current_version() {
        let dir = tempdir().unwrap();
        let m = ensure_manifest(dir.path()).unwrap();
        assert_eq!(m.product, "bradian-db");
        assert_eq!(m.version, CURRENT_FORMAT_VERSION);
        assert!(dir.path().join("format.manifest").is_file());
    }

    #[test]
    fn second_open_updates_last_opened_at() {
        let dir = tempdir().unwrap();
        let first = ensure_manifest(dir.path()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let second = ensure_manifest(dir.path()).unwrap();
        assert_eq!(first.created_at_ms, second.created_at_ms);
        assert!(second.last_opened_at_ms >= first.last_opened_at_ms);
    }

    #[test]
    fn newer_on_disk_rejected_as_too_new() {
        let dir = tempdir().unwrap();
        // Write a v999 manifest by hand.
        let fake = FormatManifest {
            product: "bradian-db".into(),
            version: 999,
            created_at_ms: 0,
            last_opened_at_ms: 0,
        };
        std::fs::write(dir.path().join("format.manifest"),
            serde_json::to_vec(&fake).unwrap()).unwrap();
        let err = ensure_manifest_with_bounds(dir.path(), 1, 1).unwrap_err();
        assert!(matches!(err, FormatError::TooNew { on_disk: 999, max: 1 }));
    }

    #[test]
    fn older_on_disk_rejected_as_too_old() {
        let dir = tempdir().unwrap();
        let fake = FormatManifest {
            product: "bradian-db".into(),
            version: 1,
            created_at_ms: 0,
            last_opened_at_ms: 0,
        };
        std::fs::write(dir.path().join("format.manifest"),
            serde_json::to_vec(&fake).unwrap()).unwrap();
        // This build requires min=5.
        let err = ensure_manifest_with_bounds(dir.path(), 5, 10).unwrap_err();
        assert!(matches!(err, FormatError::TooOld { on_disk: 1, min: 5 }));
    }

    #[test]
    fn wrong_product_rejected() {
        let dir = tempdir().unwrap();
        let fake = FormatManifest {
            product: "someone-elses-db".into(),
            version: 1,
            created_at_ms: 0,
            last_opened_at_ms: 0,
        };
        std::fs::write(dir.path().join("format.manifest"),
            serde_json::to_vec(&fake).unwrap()).unwrap();
        let err = ensure_manifest(dir.path()).unwrap_err();
        assert!(matches!(err, FormatError::WrongProduct { .. }));
    }
}
