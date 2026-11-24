//! ADR 0317 — PITR / incremental archive / offsite backup
//! **skeleton**.
//!
//! Builder validates monotonic LSN progression and stamps
//! CRCs. No WAL reading, no offsite transport, no
//! encryption, no restore.

use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArchiveChunk {
    pub min_lsn:   u64,
    pub max_lsn:   u64,
    pub crc32:     u32,
    pub bytes_len: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveManifest {
    pub archive_id:  String,
    pub min_lsn:     u64,
    pub max_lsn:     u64,
    pub chunks:      Vec<ArchiveChunk>,
    pub total_bytes: u64,
}

#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("chunk LSN must advance: last_max={last_max}, new_max={new_max}")]
    NonMonotonic { last_max: u64, new_max: u64 },
}
pub type ArchiveResult<T> = Result<T, ArchiveError>;

pub struct ArchiveBuilder {
    archive_id: String,
    from_lsn:   u64,
    chunks:     Vec<ArchiveChunk>,
    last_max:   u64,
    total:      u64,
}

impl ArchiveBuilder {
    pub fn new(archive_id: impl Into<String>, from_lsn: u64) -> Self {
        Self {
            archive_id: archive_id.into(),
            from_lsn,
            chunks:   Vec::new(),
            last_max: from_lsn,
            total:    0,
        }
    }

    pub fn add_chunk(&mut self, max_lsn: u64, payload: &[u8]) -> ArchiveResult<()> {
        if max_lsn <= self.last_max {
            return Err(ArchiveError::NonMonotonic { last_max: self.last_max, new_max: max_lsn });
        }
        let crc = crc32fast::hash(payload);
        self.chunks.push(ArchiveChunk {
            min_lsn: self.last_max + 1, max_lsn,
            crc32:   crc, bytes_len: payload.len(),
        });
        self.last_max = max_lsn;
        self.total   += payload.len() as u64;
        Ok(())
    }

    pub fn finish(self) -> ArchiveManifest {
        let max_lsn = self.chunks.last().map(|c| c.max_lsn).unwrap_or(self.from_lsn);
        ArchiveManifest {
            archive_id:  self.archive_id,
            min_lsn:     self.from_lsn,
            max_lsn,
            chunks:      self.chunks,
            total_bytes: self.total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_rejects_non_monotonic_chunk_lsn() {
        let mut b = ArchiveBuilder::new("arc-1", 100);
        b.add_chunk(150, b"first").unwrap();
        assert!(matches!(b.add_chunk(150, b"same"),
            Err(ArchiveError::NonMonotonic { .. })));
        assert!(matches!(b.add_chunk(140, b"older"),
            Err(ArchiveError::NonMonotonic { .. })));
    }

    #[test]
    fn manifest_aggregates_min_max_and_total_bytes() {
        let mut b = ArchiveBuilder::new("arc-2", 10);
        b.add_chunk(20, &[0u8; 100]).unwrap();
        b.add_chunk(30, &[0u8;  50]).unwrap();
        b.add_chunk(40, &[0u8; 200]).unwrap();
        let m = b.finish();
        assert_eq!(m.min_lsn,     10);
        assert_eq!(m.max_lsn,     40);
        assert_eq!(m.total_bytes, 350);
        assert_eq!(m.chunks.len(), 3);
        assert_eq!(m.chunks[0].min_lsn, 11);
    }

    #[test]
    fn crc_differs_for_differing_payloads() {
        let mut a = ArchiveBuilder::new("a", 0);
        let mut b = ArchiveBuilder::new("b", 0);
        a.add_chunk(1, b"foo").unwrap();
        b.add_chunk(1, b"bar").unwrap();
        assert_ne!(a.finish().chunks[0].crc32, b.finish().chunks[0].crc32);
    }
}
