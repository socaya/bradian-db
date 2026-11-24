//! ADR 0315 — DHIS2 integration deployment manifest
//! **skeleton**.
//!
//! Parses and shape-validates a small JSON manifest.
//! No HTTP fetch, no apply step, no upgrade / rollback,
//! no systemd / docker artefacts.

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
    pub ingest_cursor_path:  String,
    pub segment_retention:   RetentionPolicy,
}

pub fn load_manifest_json(bytes: &[u8]) -> BradianResult<DeploymentManifest> {
    let manifest: DeploymentManifest = serde_json::from_slice(bytes)
        .map_err(|e| BradianError::Serialization(format!("deployment manifest: {e}")))?;
    validate(&manifest)?;
    Ok(manifest)
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_manifest_round_trips() {
        let json = br#"{
            "dhis2_base_url":      "https://play.dhis2.org",
            "catalog_refresh_sec": 300,
            "ingest_cursor_path":  "/var/lib/bradian/cursor",
            "segment_retention":   { "kind": "days", "days": 90 }
        }"#;
        let m = load_manifest_json(json).unwrap();
        assert_eq!(m.catalog_refresh_sec, 300);
        assert_eq!(m.segment_retention, RetentionPolicy::Days { days: 90 });
        // Re-serialise + reparse is stable.
        let round = serde_json::to_vec(&m).unwrap();
        let again = load_manifest_json(&round).unwrap();
        assert_eq!(again, m);
    }

    #[test]
    fn invalid_url_scheme_is_rejected() {
        let json = br#"{
            "dhis2_base_url":      "ftp://dhis2.local",
            "catalog_refresh_sec": 60,
            "ingest_cursor_path":  "/tmp/cursor",
            "segment_retention":   { "kind": "forever" }
        }"#;
        let err = load_manifest_json(json).unwrap_err();
        assert!(format!("{err}").contains("http://"));
    }
}
