//! ADR 0315 — DHIS2 integration deployment path.
//!
//! A `DeploymentManifest` describes how a single Bradian
//! instance should be configured to talk to a DHIS2
//! server: the DHIS2 base URL, how often to refresh the
//! metadata catalog, where the ingest cursor lives, and
//! how long analytical segments stick around.
//!
//! This slice ships:
//! - Typed manifest + load/save JSON.
//! - `validate` that enforces URL scheme, cursor-path
//!   shape, retention sanity.
//! - `apply_to(root)` that **materialises** the manifest
//!   into an operator-ready deployment directory:
//!     `{root}/manifest.json`       — the manifest itself,
//!     `{root}/{cursor_subpath}`    — the ingest cursor
//!                                     file ensured to exist
//!                                     (parent dirs created,
//!                                     writable, 0 bytes if
//!                                     new),
//!     `{root}/retention.hint`      — a human-readable
//!                                     retention summary.
//! - `verify(root)` that re-reads an applied deployment
//!   and fails closed on drift from the recorded manifest.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use bradian_core::{BradianError, BradianResult};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum RetentionPolicy {
    Forever,
    Days     { days:  u32 },
    MaxBytes { bytes: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentManifest {
    pub dhis2_base_url:      String,
    pub catalog_refresh_sec: u32,
    /// Relative path (resolved against the deployment
    /// root) of the ingest cursor file.
    pub ingest_cursor_path:  String,
    pub segment_retention:   RetentionPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeploymentLayout {
    pub root:                  PathBuf,
    pub manifest_path:         PathBuf,
    pub ingest_cursor_path:    PathBuf,
    pub retention_hint_path:   PathBuf,
}

const MANIFEST_FILE: &str = "manifest.json";
const HINT_FILE:     &str = "retention.hint";

/// Parse + shape-validate a manifest from JSON bytes.
pub fn load_manifest_json(bytes: &[u8]) -> BradianResult<DeploymentManifest> {
    let m: DeploymentManifest = serde_json::from_slice(bytes)
        .map_err(|e| BradianError::Serialization(format!("deployment manifest: {e}")))?;
    validate(&m)?;
    Ok(m)
}

fn validate(m: &DeploymentManifest) -> BradianResult<()> {
    if !m.dhis2_base_url.starts_with("http://") && !m.dhis2_base_url.starts_with("https://") {
        return Err(BradianError::invalid_value(
            "deployment manifest: dhis2_base_url must start with http:// or https://"));
    }
    if m.catalog_refresh_sec == 0 {
        return Err(BradianError::invalid_value(
            "deployment manifest: catalog_refresh_sec must be > 0"));
    }
    if m.ingest_cursor_path.is_empty() {
        return Err(BradianError::invalid_value(
            "deployment manifest: ingest_cursor_path must not be empty"));
    }
    // Reject absolute cursor paths — they're resolved
    // against the deployment root and must stay inside.
    if Path::new(&m.ingest_cursor_path).is_absolute() {
        return Err(BradianError::invalid_value(
            "deployment manifest: ingest_cursor_path must be relative to the deployment root"));
    }
    // Reject parent-escape (`..`) components.
    if Path::new(&m.ingest_cursor_path).components()
        .any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(BradianError::invalid_value(
            "deployment manifest: ingest_cursor_path must not contain '..'"));
    }
    // Retention sanity.
    match m.segment_retention {
        RetentionPolicy::Days { days: 0 } => {
            return Err(BradianError::invalid_value(
                "deployment manifest: retention days must be > 0"));
        }
        RetentionPolicy::MaxBytes { bytes: 0 } => {
            return Err(BradianError::invalid_value(
                "deployment manifest: retention max_bytes must be > 0"));
        }
        _ => {}
    }
    Ok(())
}

impl DeploymentManifest {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("manifest serialize")
    }

    /// Materialise the manifest into `root`. Creates
    /// `root` + parent directories of the cursor file;
    /// writes the manifest JSON and a human-readable
    /// retention summary; ensures the ingest cursor file
    /// exists (zero bytes if new). Returns a
    /// `DeploymentLayout` enumerating the paths touched.
    pub fn apply_to(&self, root: impl AsRef<Path>) -> BradianResult<DeploymentLayout> {
        validate(self)?;
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)
            .map_err(|e| BradianError::invalid_value(format!("mkdir {}: {e}", root.display())))?;

        let manifest_path = root.join(MANIFEST_FILE);
        write_atomic(&manifest_path, self.to_json().as_bytes())?;

        let cursor_path = root.join(&self.ingest_cursor_path);
        if let Some(parent) = cursor_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| BradianError::invalid_value(
                    format!("mkdir {}: {e}", parent.display())))?;
        }
        // Touch the cursor file if it does not exist.
        if !cursor_path.exists() {
            let f = OpenOptions::new().write(true).create_new(true).open(&cursor_path)
                .map_err(|e| BradianError::invalid_value(
                    format!("create cursor {}: {e}", cursor_path.display())))?;
            f.sync_data().ok();
        }

        let hint_path = root.join(HINT_FILE);
        write_atomic(&hint_path, retention_hint(&self.segment_retention).as_bytes())?;

        Ok(DeploymentLayout {
            root, manifest_path, ingest_cursor_path: cursor_path, retention_hint_path: hint_path,
        })
    }
}

/// Load the manifest from an applied deployment and verify
/// its shape + cursor-path existence.
pub fn load_applied(root: &Path) -> BradianResult<(DeploymentManifest, DeploymentLayout)> {
    let manifest_path = root.join(MANIFEST_FILE);
    let file = File::open(&manifest_path)
        .map_err(|e| BradianError::invalid_value(format!("open {}: {e}",
            manifest_path.display())))?;
    let m: DeploymentManifest = serde_json::from_reader(BufReader::new(file))
        .map_err(|e| BradianError::Serialization(format!("deployment manifest: {e}")))?;
    validate(&m)?;
    let cursor = root.join(&m.ingest_cursor_path);
    if !cursor.exists() {
        return Err(BradianError::invalid_value(
            format!("ingest cursor file missing: {}", cursor.display())));
    }
    Ok((m.clone(), DeploymentLayout {
        root:                  root.to_path_buf(),
        manifest_path,
        ingest_cursor_path:    cursor,
        retention_hint_path:   root.join(HINT_FILE),
    }))
}

/// Verify a known manifest matches the applied deployment
/// bytes. Returns `Ok(())` on match, error otherwise.
pub fn verify(root: &Path, expected: &DeploymentManifest) -> BradianResult<()> {
    let (loaded, _) = load_applied(root)?;
    if &loaded != expected {
        return Err(BradianError::invalid_value(
            "applied deployment manifest drifted from expected"));
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> BradianResult<()> {
    let tmp = path.with_extension({
        let ext = path.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if ext.is_empty() { "tmp".to_string() } else { format!("{ext}.tmp") }
    });
    {
        let mut w = BufWriter::new(OpenOptions::new()
            .write(true).create(true).truncate(true).open(&tmp)
            .map_err(|e| BradianError::invalid_value(format!("open tmp {}: {e}", tmp.display())))?);
        w.write_all(bytes).map_err(|e| BradianError::invalid_value(format!("write tmp: {e}")))?;
        w.flush().map_err(|e| BradianError::invalid_value(format!("flush tmp: {e}")))?;
        w.get_ref().sync_data().ok();
    }
    std::fs::rename(&tmp, path)
        .map_err(|e| BradianError::invalid_value(format!("rename: {e}")))?;
    Ok(())
}

fn retention_hint(p: &RetentionPolicy) -> String {
    match p {
        RetentionPolicy::Forever =>
            "segment_retention = forever (segments never pruned)\n".into(),
        RetentionPolicy::Days { days } =>
            format!("segment_retention = {days} days\n"),
        RetentionPolicy::MaxBytes { bytes } =>
            format!("segment_retention = {bytes} max bytes\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_manifest_round_trips() {
        let json = br#"{
            "dhis2_base_url":      "https://play.dhis2.org",
            "catalog_refresh_sec": 300,
            "ingest_cursor_path":  "cursor/ingest.cursor",
            "segment_retention":   { "kind": "days", "days": 90 }
        }"#;
        let m = load_manifest_json(json).unwrap();
        assert_eq!(m.catalog_refresh_sec, 300);
        assert_eq!(m.segment_retention, RetentionPolicy::Days { days: 90 });
        let round = serde_json::to_vec(&m).unwrap();
        let again = load_manifest_json(&round).unwrap();
        assert_eq!(again, m);
    }

    #[test]
    fn invalid_url_scheme_rejected() {
        let json = br#"{
            "dhis2_base_url":      "ftp://dhis2.local",
            "catalog_refresh_sec": 60,
            "ingest_cursor_path":  "cursor",
            "segment_retention":   { "kind": "forever" }
        }"#;
        let err = load_manifest_json(json).unwrap_err();
        assert!(format!("{err}").contains("http://"));
    }

    #[test]
    fn absolute_cursor_path_rejected() {
        let m = DeploymentManifest {
            dhis2_base_url:      "https://x".into(),
            catalog_refresh_sec: 60,
            ingest_cursor_path:  "/etc/passwd".into(),
            segment_retention:   RetentionPolicy::Forever,
        };
        let err = validate(&m).unwrap_err();
        assert!(format!("{err}").contains("relative"));
    }

    #[test]
    fn parent_escape_in_cursor_path_rejected() {
        let m = DeploymentManifest {
            dhis2_base_url:      "https://x".into(),
            catalog_refresh_sec: 60,
            ingest_cursor_path:  "../escape".into(),
            segment_retention:   RetentionPolicy::Forever,
        };
        let err = validate(&m).unwrap_err();
        assert!(format!("{err}").contains("'..'"));
    }

    #[test]
    fn zero_retention_values_rejected() {
        let mut m = DeploymentManifest {
            dhis2_base_url:      "https://x".into(),
            catalog_refresh_sec: 60,
            ingest_cursor_path:  "cursor".into(),
            segment_retention:   RetentionPolicy::Days { days: 0 },
        };
        assert!(validate(&m).is_err());
        m.segment_retention = RetentionPolicy::MaxBytes { bytes: 0 };
        assert!(validate(&m).is_err());
    }

    #[test]
    fn apply_to_materialises_manifest_cursor_and_hint() {
        let dir = tempfile::tempdir().unwrap();
        let m = DeploymentManifest {
            dhis2_base_url:      "https://play.dhis2.org".into(),
            catalog_refresh_sec: 300,
            ingest_cursor_path:  "cursors/ingest.cursor".into(),
            segment_retention:   RetentionPolicy::Days { days: 90 },
        };
        let layout = m.apply_to(dir.path()).unwrap();
        assert!(layout.manifest_path.is_file());
        assert!(layout.ingest_cursor_path.is_file());
        assert!(layout.retention_hint_path.is_file());
        // Cursor file was created new, 0 bytes.
        assert_eq!(std::fs::metadata(&layout.ingest_cursor_path).unwrap().len(), 0);
        // Retention hint carries the human-readable value.
        let hint = std::fs::read_to_string(&layout.retention_hint_path).unwrap();
        assert!(hint.contains("90 days"));
        // load_applied re-reads everything.
        let (loaded, _) = load_applied(dir.path()).unwrap();
        assert_eq!(loaded, m);
    }

    #[test]
    fn apply_is_idempotent_and_preserves_existing_cursor_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let m = DeploymentManifest {
            dhis2_base_url:      "https://host".into(),
            catalog_refresh_sec: 60,
            ingest_cursor_path:  "cur".into(),
            segment_retention:   RetentionPolicy::Forever,
        };
        let layout = m.apply_to(dir.path()).unwrap();
        // User writes their own bytes into the cursor.
        std::fs::write(&layout.ingest_cursor_path, b"lsn=12345").unwrap();
        // Second apply must not clobber the cursor.
        m.apply_to(dir.path()).unwrap();
        let after = std::fs::read(&layout.ingest_cursor_path).unwrap();
        assert_eq!(after, b"lsn=12345");
    }

    #[test]
    fn verify_detects_drift_between_known_and_applied_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let m = DeploymentManifest {
            dhis2_base_url:      "https://a".into(),
            catalog_refresh_sec: 60,
            ingest_cursor_path:  "cur".into(),
            segment_retention:   RetentionPolicy::Forever,
        };
        m.apply_to(dir.path()).unwrap();
        verify(dir.path(), &m).unwrap();
        // Now rewrite the manifest file with different bytes.
        let other = DeploymentManifest {
            dhis2_base_url:      "https://b".into(), ..m.clone()
        };
        other.apply_to(dir.path()).unwrap();
        let err = verify(dir.path(), &m).unwrap_err();
        assert!(format!("{err}").contains("drifted"));
    }

    #[test]
    fn load_applied_rejects_missing_cursor_file() {
        let dir = tempfile::tempdir().unwrap();
        let m = DeploymentManifest {
            dhis2_base_url:      "https://a".into(),
            catalog_refresh_sec: 60,
            ingest_cursor_path:  "cur".into(),
            segment_retention:   RetentionPolicy::Forever,
        };
        m.apply_to(dir.path()).unwrap();
        // Operator removes the cursor file; load_applied
        // fails closed.
        std::fs::remove_file(dir.path().join("cur")).unwrap();
        let err = load_applied(dir.path()).unwrap_err();
        assert!(format!("{err}").contains("ingest cursor file missing"));
    }
}
