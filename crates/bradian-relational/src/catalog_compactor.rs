//! ADR 0119 — catalog journal compactor (BRD-REL-001).
//!
//! `PersistentCatalog` is append-only forever. Over a long-
//! running deployment the DDL journal grows unbounded. This
//! module ships a compactor that writes the current catalog
//! snapshot as a single "snapshot" record and truncates the
//! journal after it.
//!
//! Honest scope: the compactor is a STAGING helper. Engines
//! call it offline (no live writers) and it produces a NEW
//! journal file under a temporary name. Operators can then
//! swap the file in with a manual rename or a follow-up ADR
//! can wire it into the lifecycle. The current
//! `PersistentCatalog::open()` continues to replay the
//! full journal — so compaction is INERT until the open path
//! is taught to recognise the snapshot record.

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::Catalog;

const SNAPSHOT_MAGIC: &[u8; 8] = b"BCAT-SNP";

/// Serializable snapshot of a Catalog's decoded state. The
/// compactor writes this as a single record that replaces N
/// DDL events.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatalogSnapshot {
    pub version: u32,
    pub payload: Catalog,
}

#[derive(Debug, Error)]
pub enum CompactorError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, CompactorError>;

/// Write a `CatalogSnapshot` to `destination` under a fresh
/// file. Does NOT modify the source journal. The destination
/// file layout:
///
///   [0..8]   magic "BCAT-SNP"
///   [8..12]  u32 LE: JSON body length
///   [12..]   JSON body (serde_json)
///   [end-4..end] u32 LE: CRC32 over header + body
pub fn write_snapshot(destination: &Path, snapshot: &CatalogSnapshot) -> Result<()> {
    let body = serde_json::to_vec(snapshot)?;
    let body_len = u32::try_from(body.len())
        .map_err(|_| CompactorError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput, "catalog snapshot exceeds u32::MAX",
        )))?;
    let mut out = Vec::with_capacity(8 + 4 + body.len() + 4);
    out.extend_from_slice(SNAPSHOT_MAGIC);
    out.extend_from_slice(&body_len.to_le_bytes());
    out.extend_from_slice(&body);
    let crc = crc32fast::hash(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    let tmp = destination.with_extension("compact.tmp");
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&out)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, destination)?;
    Ok(())
}

/// Read a snapshot written by `write_snapshot`. Verifies CRC.
pub fn read_snapshot(source: &Path) -> Result<CatalogSnapshot> {
    let bytes = std::fs::read(source)?;
    if bytes.len() < 8 + 4 + 4 {
        return Err(CompactorError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData, "snapshot file too short",
        )));
    }
    if &bytes[..8] != SNAPSHOT_MAGIC {
        return Err(CompactorError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData, "snapshot magic mismatch",
        )));
    }
    let body_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let payload_end = 12 + body_len;
    if bytes.len() != payload_end + 4 {
        return Err(CompactorError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData, "snapshot file size does not match declared body length",
        )));
    }
    let declared = u32::from_le_bytes(bytes[payload_end..payload_end + 4].try_into().unwrap());
    let actual = crc32fast::hash(&bytes[..payload_end]);
    if declared != actual {
        return Err(CompactorError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData, "snapshot crc mismatch",
        )));
    }
    let snap: CatalogSnapshot = serde_json::from_slice(&bytes[12..payload_end])?;
    Ok(snap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Catalog;
    use tempfile::tempdir;

    fn fixture_catalog() -> Catalog {
        let mut cat = Catalog::new();
        let db = cat.create_database("test").unwrap();
        let schema = cat.create_schema(db, "public").unwrap();
        cat.create_table(schema, crate::model::TableSpec {
            name: "items".into(),
            columns: vec![crate::model::Column {
                name: "id".into(),
                data_type: crate::model::DataType::Int64,
                nullable: false,
            }],
            primary_key: crate::model::PrimaryKey { columns: vec!["id".into()] },
            unique_keys: vec![],
        }).unwrap();
        cat
    }

    #[test]
    fn write_and_read_round_trip_preserves_payload() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("snap.bin");
        let snap = CatalogSnapshot { version: 1, payload: fixture_catalog() };
        write_snapshot(&path, &snap).unwrap();
        let back = read_snapshot(&path).unwrap();
        // Compare via re-encoding (Catalog doesn't derive Eq).
        assert_eq!(serde_json::to_string(&back.payload).unwrap(),
                   serde_json::to_string(&snap.payload).unwrap());
    }

    #[test]
    fn missing_magic_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bad.bin");
        std::fs::write(&path, b"NOT_A_SNAPSHOT_FILE_12345").unwrap();
        assert!(read_snapshot(&path).is_err());
    }

    #[test]
    fn crc_tamper_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("snap.bin");
        let snap = CatalogSnapshot { version: 1, payload: fixture_catalog() };
        write_snapshot(&path, &snap).unwrap();
        // Flip a body byte.
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[20] ^= 0xFF;
        std::fs::write(&path, &bytes).unwrap();
        assert!(read_snapshot(&path).is_err());
    }

    #[test]
    fn empty_catalog_round_trips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("empty.bin");
        let snap = CatalogSnapshot { version: 1, payload: Catalog::new() };
        write_snapshot(&path, &snap).unwrap();
        let _ = read_snapshot(&path).unwrap();
    }
}
