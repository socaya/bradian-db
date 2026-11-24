//! ADR 0318 — Schema history compaction with reference proofs.
//!
//! Operators periodically compact the schema history
//! journal to drop versions no retained row depends on.
//! A compaction that silently dropped a still-referenced
//! version would break historical reads; a compaction
//! whose output can't be independently verified by a
//! reviewer is operationally untrustworthy.
//!
//! This slice ships a **real on-disk journal rewrite**:
//! read a journal file, split entries into retained /
//! dropped by a live-set, atomically replace the file
//! with the compacted copy, and return a BLAKE3 proof
//! over the retained entries. A reviewer can later
//! re-read the compacted journal and recompute the proof
//! to confirm it agrees with what the operator kept.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SchemaVersion {
    pub version: u64,
    /// Content-addressed digest of this schema version
    /// (caller-supplied; typically BLAKE3 of the
    /// serialised DDL event bytes).
    #[serde(with = "serde_32")]
    pub digest:  [u8; 32],
}

mod serde_32 {
    use serde::{Deserializer, Serializer, Deserialize};
    pub fn serialize<S: Serializer>(v: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::hex::encode(v))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let s = String::deserialize(d)?;
        super::hex::decode32(&s).ok_or_else(||
            serde::de::Error::custom("digest must be 64 hex chars"))
    }
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        const H: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            out.push(H[(*b >> 4) as usize] as char);
            out.push(H[(*b & 0x0f) as usize] as char);
        }
        out
    }
    pub fn decode32(s: &str) -> Option<[u8; 32]> {
        if s.len() != 64 || !s.chars().all(|c| c.is_ascii_hexdigit()) { return None; }
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = u8::from_str_radix(&s[i*2..i*2+2], 16).ok()?;
        }
        Some(out)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaJournal {
    pub journal_version: u32,
    pub entries:         Vec<SchemaVersion>,
}

impl SchemaJournal {
    pub fn new(entries: Vec<SchemaVersion>) -> Self {
        Self { journal_version: 1, entries }
    }

    pub fn read_from(path: &Path) -> CompactionResult<Self> {
        let f = File::open(path)?;
        let j: SchemaJournal = serde_json::from_reader(BufReader::new(f))?;
        if j.journal_version != 1 {
            return Err(CompactionError::UnsupportedVersion(j.journal_version));
        }
        Ok(j)
    }

    pub fn write_to(&self, path: &Path) -> CompactionResult<()> {
        let mut w = BufWriter::new(OpenOptions::new()
            .write(true).create(true).truncate(true).open(path)?);
        serde_json::to_writer_pretty(&mut w, self)?;
        w.flush()?;
        w.get_ref().sync_data()?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactionPlan {
    pub retained: Vec<SchemaVersion>,
    pub dropped:  Vec<SchemaVersion>,
    /// BLAKE3-256 digest covering `(version‖digest)` of
    /// each retained entry in ascending-version order.
    pub proof:    [u8; 32],
}

impl CompactionPlan {
    pub fn proof_hex(&self) -> String { hex::encode(&self.proof) }
}

#[derive(Debug, Error)]
pub enum CompactionError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("unsupported journal version: {0}")]
    UnsupportedVersion(u32),
    #[error("proof mismatch: expected {expected}, actual {actual}")]
    ProofMismatch { expected: String, actual: String },
}
pub type CompactionResult<T> = Result<T, CompactionError>;

/// Plan a compaction over an in-memory entry set.
pub fn plan_compaction(all: &[SchemaVersion], live: &[u64]) -> CompactionPlan {
    let live_set: HashSet<u64> = live.iter().copied().collect();
    let mut retained = Vec::new();
    let mut dropped  = Vec::new();
    for v in all {
        if live_set.contains(&v.version) { retained.push(*v); }
        else                             { dropped.push(*v);  }
    }
    retained.sort_by_key(|v| v.version);
    dropped.sort_by_key (|v| v.version);
    let proof = compute_proof(&retained);
    CompactionPlan { retained, dropped, proof }
}

fn compute_proof(retained: &[SchemaVersion]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    for v in retained {
        h.update(&v.version.to_le_bytes());
        h.update(&v.digest);
    }
    *h.finalize().as_bytes()
}

/// Verify the plan's proof matches the retained entries.
pub fn verify_proof(plan: &CompactionPlan) -> bool {
    compute_proof(&plan.retained) == plan.proof
}

/// Compact an on-disk journal in place: tmp-write, sync,
/// atomic rename. Returns the plan; the caller persists
/// `plan.proof` outside the journal so a reviewer can
/// later `verify_file_proof`.
pub fn compact_file_in_place(
    journal_path: &Path, live: &[u64],
) -> CompactionResult<CompactionPlan> {
    let journal = SchemaJournal::read_from(journal_path)?;
    let plan    = plan_compaction(&journal.entries, live);
    let tmp     = tmp_path(journal_path);
    SchemaJournal::new(plan.retained.clone()).write_to(&tmp)?;
    std::fs::rename(&tmp, journal_path)?;
    #[cfg(unix)]
    if let Some(dir) = journal_path.parent() {
        if let Ok(d) = File::open(dir) { let _ = d.sync_data(); }
    }
    Ok(plan)
}

/// Verify a prior proof against a journal on disk.
pub fn verify_file_proof(
    journal_path: &Path, expected_proof: &[u8; 32],
) -> CompactionResult<()> {
    let journal = SchemaJournal::read_from(journal_path)?;
    let actual  = compute_proof(&journal.entries);
    if &actual != expected_proof {
        return Err(CompactionError::ProofMismatch {
            expected: hex::encode(expected_proof),
            actual:   hex::encode(&actual),
        });
    }
    Ok(())
}

pub fn tmp_path(journal_path: &Path) -> PathBuf {
    journal_path.with_file_name(format!(
        "{}.tmp", journal_path.file_name().unwrap().to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(v: u64, fill: u8) -> SchemaVersion {
        SchemaVersion { version: v, digest: [fill; 32] }
    }

    #[test]
    fn plan_partitions_entries_and_verify_proof_agrees() {
        let all = vec![version(1, 0xa), version(2, 0xb), version(3, 0xc), version(4, 0xd)];
        let plan = plan_compaction(&all, &[2, 4]);
        assert_eq!(plan.retained, vec![version(2, 0xb), version(4, 0xd)]);
        assert_eq!(plan.dropped,  vec![version(1, 0xa), version(3, 0xc)]);
        assert!(verify_proof(&plan));
    }

    #[test]
    fn tampering_with_retained_fails_proof() {
        let all = vec![version(1, 0xaa), version(2, 0xbb)];
        let mut plan = plan_compaction(&all, &[1, 2]);
        plan.retained[0].digest[0] ^= 0xff;
        assert!(!verify_proof(&plan));
    }

    #[test]
    fn compact_file_in_place_atomically_rewrites_the_journal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.json");
        SchemaJournal::new(vec![
            version(1, 1), version(2, 2), version(3, 3), version(4, 4),
        ]).write_to(&path).unwrap();
        let plan = compact_file_in_place(&path, &[2, 4]).unwrap();
        assert_eq!(plan.retained.len(), 2);
        let reread = SchemaJournal::read_from(&path).unwrap();
        assert_eq!(reread.entries, vec![version(2, 2), version(4, 4)]);
        verify_file_proof(&path, &plan.proof).unwrap();
        assert!(!tmp_path(&path).exists(),
            "tmp file must be renamed away");
    }

    #[test]
    fn verify_file_proof_rejects_a_tampered_journal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.json");
        SchemaJournal::new(vec![version(1, 1), version(2, 2)])
            .write_to(&path).unwrap();
        let plan = plan_compaction(
            &[version(1, 1), version(2, 2)], &[1, 2]);
        verify_file_proof(&path, &plan.proof).unwrap();
        SchemaJournal::new(vec![version(1, 1), version(2, 0xff)])
            .write_to(&path).unwrap();
        let err = verify_file_proof(&path, &plan.proof).unwrap_err();
        assert!(matches!(err, CompactionError::ProofMismatch { .. }));
    }

    #[test]
    fn read_rejects_unsupported_journal_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path,
            br#"{"journal_version":42,"entries":[]}"#).unwrap();
        let err = SchemaJournal::read_from(&path).unwrap_err();
        assert!(matches!(err, CompactionError::UnsupportedVersion(42)));
    }

    #[test]
    fn write_round_trip_preserves_hex_digest_encoding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rt.json");
        let j = SchemaJournal::new(vec![
            SchemaVersion { version: 7, digest: [0xab; 32] },
            SchemaVersion { version: 8, digest: [0xcd; 32] },
        ]);
        j.write_to(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(&"ab".repeat(32)));
        assert!(text.contains(&"cd".repeat(32)));
        let back = SchemaJournal::read_from(&path).unwrap();
        assert_eq!(back, j);
    }
}
