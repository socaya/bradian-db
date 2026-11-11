//! Partition manifest — small JSON document, atomically published.
//!
//! Manifests are written via temp-and-rename. A CRC32 trailer protects against partial writes.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use dap_core::{DapError, DapResult, Fingerprint};
use serde::{Deserialize, Serialize};

use crate::checksum::crc32;

/// A visible segment recorded by the manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestSegment {
    pub segment_id: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub row_count: u64,
    pub watermark_ms: i64,
    pub data_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub generation: u64,
    pub visible_segments: Vec<ManifestSegment>,
    /// Paths to delete-vector files (STO-006). Empty in v1 — compaction only flushes delta.
    #[serde(default)]
    pub tombstones: Vec<PathBuf>,
    pub wal_truncation_lsn: u64,
    pub schema_fingerprint: Fingerprint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_revision: Option<Fingerprint>,
}

impl Manifest {
    pub fn new(schema_fingerprint: Fingerprint) -> Self {
        Self {
            generation: 0,
            visible_segments: Vec::new(),
            tombstones: Vec::new(),
            wal_truncation_lsn: 0,
            schema_fingerprint,
            metadata_revision: None,
        }
    }

    /// Load from disk. Prefers `manifest.json`; falls back to `manifest.json.bak` if the primary
    /// is corrupt or missing. Returns `Ok(None)` if neither exists (fresh partition).
    pub fn load(dir: &Path) -> DapResult<Option<Self>> {
        let primary = dir.join("manifest.json");
        let backup = dir.join("manifest.json.bak");
        for candidate in [&primary, &backup] {
            if candidate.exists() {
                match Self::read_file(candidate) {
                    Ok(m) => return Ok(Some(m)),
                    Err(e) => {
                        tracing::warn!(?candidate, error = %e, "manifest candidate rejected");
                    }
                }
            }
        }
        if primary.exists() || backup.exists() {
            return Err(DapError::corruption(
                "both manifest copies are corrupt or unreadable",
            ));
        }
        Ok(None)
    }

    /// Atomically publish `self` as the new manifest for the partition directory.
    pub fn save(&self, dir: &Path) -> DapResult<()> {
        std::fs::create_dir_all(dir)?;
        let primary = dir.join("manifest.json");
        let backup = dir.join("manifest.json.bak");
        let tmp = dir.join("manifest.json.tmp");

        // Build body: JSON + 8-byte little-endian CRC32 trailer.
        let json = serde_json::to_vec_pretty(self)?;
        let crc = crc32(&json);
        let mut body = json;
        body.extend_from_slice(&crc.to_le_bytes());
        body.extend_from_slice(&[0u8; 4]); // reserved — pad to 8 so trailer is self-describing

        // Rotate primary -> backup before overwriting, best-effort.
        if primary.exists() {
            // Ignore rename-to-backup failures; the write-and-rename of primary remains atomic.
            let _ = std::fs::rename(&primary, &backup);
        }

        {
            let mut f = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp)?;
            f.write_all(&body)?;
            f.flush()?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &primary)?;
        if let Ok(dir_file) = File::open(dir) {
            let _ = dir_file.sync_all();
        }
        Ok(())
    }

    fn read_file(path: &Path) -> DapResult<Self> {
        let mut bytes = Vec::new();
        File::open(path)?.read_to_end(&mut bytes)?;
        if bytes.len() < 8 {
            return Err(DapError::corruption("manifest too short"));
        }
        let (json_bytes, trailer) = bytes.split_at(bytes.len() - 8);
        let crc = u32::from_le_bytes(trailer[0..4].try_into().unwrap());
        if crc32(json_bytes) != crc {
            return Err(DapError::corruption("manifest crc mismatch"));
        }
        let m: Manifest = serde_json::from_slice(json_bytes)?;
        Ok(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fp() -> Fingerprint {
        Fingerprint::of(b"schema")
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempdir().unwrap();
        let m = Manifest {
            generation: 7,
            visible_segments: vec![ManifestSegment {
                segment_id: "s1".into(),
                path: dir.path().join("segments/s1.derc"),
                size_bytes: 1024,
                row_count: 42,
                watermark_ms: 1000,
                data_revision: 3,
            }],
            tombstones: vec![],
            wal_truncation_lsn: 10,
            schema_fingerprint: fp(),
            metadata_revision: None,
        };
        m.save(dir.path()).unwrap();
        let back = Manifest::load(dir.path()).unwrap().unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn corrupt_primary_falls_back_to_backup() {
        let dir = tempdir().unwrap();
        let m1 = Manifest {
            generation: 1,
            visible_segments: vec![],
            tombstones: vec![],
            wal_truncation_lsn: 0,
            schema_fingerprint: fp(),
            metadata_revision: None,
        };
        m1.save(dir.path()).unwrap();
        let m2 = Manifest {
            generation: 2,
            visible_segments: vec![],
            tombstones: vec![],
            wal_truncation_lsn: 10,
            schema_fingerprint: fp(),
            metadata_revision: None,
        };
        m2.save(dir.path()).unwrap();

        // Corrupt primary; backup still holds m1.
        let primary = dir.path().join("manifest.json");
        let mut bytes = std::fs::read(&primary).unwrap();
        bytes[0] ^= 0xFF;
        std::fs::write(&primary, bytes).unwrap();

        let back = Manifest::load(dir.path()).unwrap().unwrap();
        assert_eq!(back.generation, 1, "fell back to backup");
    }

    #[test]
    fn missing_manifest_returns_none() {
        let dir = tempdir().unwrap();
        let back = Manifest::load(dir.path()).unwrap();
        assert!(back.is_none());
    }
}
