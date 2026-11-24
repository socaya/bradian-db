//! ADR 0317 — PITR / incremental archive / offsite backup.
//!
//! Reads the RowStore's WAL file as an append-only byte
//! stream starting at a caller-supplied offset, chunks the
//! bytes under a size cap, writes each chunk + a JSON
//! manifest to a target directory, and provides a restore
//! path that stitches the chunks back into a WAL file in a
//! fresh store directory. The resulting directory, once
//! reopened by `RowStore::open`, replays the WAL to
//! reconstruct the archived state.
//!
//! The model is deliberately **byte-offset-based**, not
//! LSN-based: the WAL frames are self-describing and the
//! store's existing recovery path knows how to parse them.
//! The archive need only preserve the byte sequence
//! verbatim. That makes the round-trip provable end-to-end
//! without reaching into WAL frame internals.
//!
//! Scope of this slice:
//! - Full WAL (`from_wal_offset = 0`) and incremental
//!   (`from_wal_offset > 0`) archives.
//! - Chunking under a caller-supplied `chunk_bytes` cap.
//! - CRC32 per chunk + whole-archive verification.
//! - Restore into an empty target directory, producing
//!   `rows.wal` ready for `RowStore::open`.
//! - Incremental apply onto an existing target WAL that
//!   already contains the prior-archive bytes (verifies
//!   the current size matches `from_wal_offset`).
//! - Offsite transport is out of scope — this ships the
//!   bytes on the local filesystem; `cp`/`rsync`/object
//!   store copy is a caller concern.
//! - Encryption of chunks is out of scope.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Default per-chunk cap (16 MiB). Callers tune via
/// `ArchiveOptions::chunk_bytes`.
pub const DEFAULT_CHUNK_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct ArchiveOptions {
    pub chunk_bytes: usize,
}

impl Default for ArchiveOptions {
    fn default() -> Self { Self { chunk_bytes: DEFAULT_CHUNK_BYTES } }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveChunk {
    pub index:        u32,
    pub start_offset: u64,
    pub end_offset:   u64,
    pub crc32:        u32,
    pub file:         String,       // "chunk-00001.bin"
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveManifest {
    pub archive_id:       String,
    pub wal_file_name:    String,   // "rows.wal"
    pub from_wal_offset:  u64,
    pub to_wal_offset:    u64,
    pub total_bytes:      u64,
    pub chunks:           Vec<ArchiveChunk>,
    pub manifest_version: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArchiveReceipt {
    pub from_wal_offset: u64,
    pub to_wal_offset:   u64,
    pub chunks_written:  u32,
    pub total_bytes:     u64,
}

#[derive(Debug, Error)]
pub enum ArchiveError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("chunk_bytes must be > 0")]
    ZeroChunkBytes,
    #[error("from_wal_offset {from} exceeds current WAL length {len}")]
    OffsetPastEnd { from: u64, len: u64 },
    #[error("target directory is not empty: {0:?}")]
    TargetNotEmpty(PathBuf),
    #[error("manifest version {0} is not supported")]
    UnsupportedManifest(u32),
    #[error("chunk {index} CRC mismatch: stored={stored:08x} computed={computed:08x}")]
    ChunkCrc { index: u32, stored: u32, computed: u32 },
    #[error("chunk {0} missing from archive directory")]
    ChunkMissing(u32),
    #[error("apply_to_wal: target WAL length {target_len} != manifest from_offset {expected}")]
    ApplyMisalignment { target_len: u64, expected: u64 },
    #[error("chunk ordering gap: expected start {expected}, got {got}")]
    ChunkGap { expected: u64, got: u64 },
}

pub type ArchiveResult<T> = Result<T, ArchiveError>;

/// Write an incremental archive from a RowStore directory
/// (identified by its WAL file path) into `out_dir`. The
/// output directory is created if missing; if it contains
/// existing files, the archive is refused (ADR 0014's
/// publication rule — never silently overwrite).
pub fn write_from_wal(
    wal_path:        &Path,
    from_wal_offset: u64,
    archive_id:      impl Into<String>,
    out_dir:         &Path,
    options:         ArchiveOptions,
) -> ArchiveResult<(ArchiveManifest, ArchiveReceipt)> {
    if options.chunk_bytes == 0 { return Err(ArchiveError::ZeroChunkBytes); }

    // Create the output directory and reject non-empty
    // existing directories. A missing dir is created fresh.
    match std::fs::read_dir(out_dir) {
        Ok(mut it) => if it.next().is_some() {
            return Err(ArchiveError::TargetNotEmpty(out_dir.to_path_buf())); },
        Err(e) if e.kind() == ErrorKind::NotFound => {
            std::fs::create_dir_all(out_dir)?;
        }
        Err(e) => return Err(e.into()),
    }

    let mut wal = BufReader::new(File::open(wal_path)?);
    let wal_len = wal.seek(SeekFrom::End(0))?;
    if from_wal_offset > wal_len {
        return Err(ArchiveError::OffsetPastEnd { from: from_wal_offset, len: wal_len });
    }
    wal.seek(SeekFrom::Start(from_wal_offset))?;

    let wal_file_name = wal_path.file_name()
        .and_then(|s| s.to_str()).unwrap_or("rows.wal").to_string();

    let mut chunks       = Vec::new();
    let mut buf          = vec![0u8; options.chunk_bytes];
    let mut current_off  = from_wal_offset;
    let mut chunk_index  = 0u32;
    let mut total_bytes  = 0u64;

    loop {
        // Fill buf up to chunk_bytes or EOF.
        let mut read_into = 0usize;
        while read_into < buf.len() {
            let n = wal.read(&mut buf[read_into..])?;
            if n == 0 { break; }
            read_into += n;
        }
        if read_into == 0 { break; }

        let slice   = &buf[..read_into];
        let crc     = crc32fast::hash(slice);
        let file    = format!("chunk-{:05}.bin", chunk_index);
        let path    = out_dir.join(&file);
        let mut w   = BufWriter::new(OpenOptions::new()
            .write(true).create_new(true).open(&path)?);
        w.write_all(slice)?;
        w.flush()?;
        w.get_ref().sync_data()?;

        let start = current_off;
        let end   = current_off + read_into as u64;
        chunks.push(ArchiveChunk {
            index: chunk_index, start_offset: start, end_offset: end,
            crc32: crc, file,
        });
        current_off  = end;
        total_bytes += read_into as u64;
        chunk_index += 1;
    }

    let manifest = ArchiveManifest {
        archive_id: archive_id.into(),
        wal_file_name,
        from_wal_offset,
        to_wal_offset:    current_off,
        total_bytes,
        chunks,
        manifest_version: 1,
    };

    let manifest_path = out_dir.join("manifest.json");
    let mut mw = BufWriter::new(OpenOptions::new()
        .write(true).create_new(true).open(&manifest_path)?);
    serde_json::to_writer_pretty(&mut mw, &manifest)?;
    mw.flush()?;
    mw.get_ref().sync_data()?;

    let receipt = ArchiveReceipt {
        from_wal_offset,
        to_wal_offset:   current_off,
        chunks_written:  chunk_index,
        total_bytes,
    };
    Ok((manifest, receipt))
}

pub fn read_manifest(archive_dir: &Path) -> ArchiveResult<ArchiveManifest> {
    let f = File::open(archive_dir.join("manifest.json"))?;
    let m: ArchiveManifest = serde_json::from_reader(BufReader::new(f))?;
    if m.manifest_version != 1 {
        return Err(ArchiveError::UnsupportedManifest(m.manifest_version));
    }
    Ok(m)
}

/// Walk every chunk in the manifest, re-read it from
/// `archive_dir` and verify its CRC and byte length.
pub fn verify(archive_dir: &Path) -> ArchiveResult<ArchiveReceipt> {
    let manifest = read_manifest(archive_dir)?;
    let mut expected_start = manifest.from_wal_offset;
    let mut total = 0u64;
    for chunk in &manifest.chunks {
        if chunk.start_offset != expected_start {
            return Err(ArchiveError::ChunkGap {
                expected: expected_start, got: chunk.start_offset });
        }
        let chunk_path = archive_dir.join(&chunk.file);
        let bytes = match std::fs::read(&chunk_path) {
            Ok(b) => b,
            Err(e) if e.kind() == ErrorKind::NotFound =>
                return Err(ArchiveError::ChunkMissing(chunk.index)),
            Err(e) => return Err(e.into()),
        };
        let expected_len = (chunk.end_offset - chunk.start_offset) as usize;
        if bytes.len() != expected_len {
            return Err(ArchiveError::ChunkCrc {
                index: chunk.index, stored: chunk.crc32,
                computed: crc32fast::hash(&bytes),
            });
        }
        let crc = crc32fast::hash(&bytes);
        if crc != chunk.crc32 {
            return Err(ArchiveError::ChunkCrc {
                index: chunk.index, stored: chunk.crc32, computed: crc });
        }
        total += bytes.len() as u64;
        expected_start = chunk.end_offset;
    }
    Ok(ArchiveReceipt {
        from_wal_offset: manifest.from_wal_offset,
        to_wal_offset:   manifest.to_wal_offset,
        chunks_written:  manifest.chunks.len() as u32,
        total_bytes:     total,
    })
}

/// Apply the archive onto a target WAL file. The target is
/// opened in append mode; its current length must equal
/// the manifest's `from_wal_offset` so this call only ever
/// appends the archived range. The final file length ends
/// at `to_wal_offset`.
///
/// Use `from_wal_offset == 0` for a full restore into an
/// empty directory (this call creates the file).
pub fn apply_to_wal(archive_dir: &Path, target_wal: &Path) -> ArchiveResult<ArchiveReceipt> {
    let manifest = read_manifest(archive_dir)?;
    let receipt  = verify(archive_dir)?;   // fail-closed before any append

    // Ensure parent exists.
    if let Some(parent) = target_wal.parent() { std::fs::create_dir_all(parent)?; }

    let (mut writer, current_len) = match OpenOptions::new()
        .append(true).open(target_wal) {
        Ok(f) => {
            let len = f.metadata()?.len();
            (BufWriter::new(f), len)
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            if manifest.from_wal_offset != 0 {
                return Err(ArchiveError::ApplyMisalignment {
                    target_len: 0, expected: manifest.from_wal_offset });
            }
            (BufWriter::new(OpenOptions::new()
                .write(true).create_new(true).open(target_wal)?), 0u64)
        }
        Err(e) => return Err(e.into()),
    };

    if current_len != manifest.from_wal_offset {
        return Err(ArchiveError::ApplyMisalignment {
            target_len: current_len, expected: manifest.from_wal_offset });
    }

    for chunk in &manifest.chunks {
        let bytes = std::fs::read(archive_dir.join(&chunk.file))?;
        writer.write_all(&bytes)?;
    }
    writer.flush()?;
    writer.get_ref().sync_data()?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::row_store::RowStore;

    fn write_rows(store: &RowStore, range: std::ops::Range<u64>) {
        for i in range {
            let key = i.to_be_bytes().to_vec();
            let value = format!("v{i}").into_bytes();
            store.put(&key, &value).unwrap();
        }
    }

    #[test]
    fn full_archive_round_trips_into_a_fresh_store_directory() {
        let src = tempfile::tempdir().unwrap();
        let src_store = RowStore::open(src.path()).unwrap();
        write_rows(&src_store, 0..100);
        let src_lsn = src_store.last_lsn();
        drop(src_store);

        let archive = tempfile::tempdir().unwrap();
        let (manifest, receipt) = write_from_wal(
            &src.path().join("rows.wal"), 0, "full-arc",
            archive.path(), ArchiveOptions { chunk_bytes: 4096 },
        ).unwrap();
        assert_eq!(receipt.from_wal_offset, 0);
        assert_eq!(receipt.to_wal_offset, manifest.to_wal_offset);
        assert!(receipt.chunks_written >= 1);

        // Verification pass must agree.
        let v = verify(archive.path()).unwrap();
        assert_eq!(v, receipt);

        // Apply to a new, empty target dir. The restored
        // WAL should reopen and yield the same rows + LSN.
        let dst = tempfile::tempdir().unwrap();
        apply_to_wal(archive.path(), &dst.path().join("rows.wal")).unwrap();
        let dst_store = RowStore::open(dst.path()).unwrap();
        assert_eq!(dst_store.last_lsn(), src_lsn);
        for i in 0..100u64 {
            assert_eq!(
                dst_store.get(&i.to_be_bytes()),
                Some(format!("v{i}").into_bytes()),
                "row {i} did not round-trip through archive",
            );
        }
    }

    #[test]
    fn incremental_archive_appends_only_the_new_wal_range() {
        let src = tempfile::tempdir().unwrap();
        let wal_path = src.path().join("rows.wal");

        // Phase 1: write 0..50 and take a full archive of
        // the WAL AS IT STANDS at that moment. The resulting
        // archive ends at `split_offset`.
        {
            let store = RowStore::open(src.path()).unwrap();
            write_rows(&store, 0..50);
        }   // drop(store) — ensures recovery-side frames have landed
        let split_offset = std::fs::metadata(&wal_path).unwrap().len();
        let arc_full = tempfile::tempdir().unwrap();
        write_from_wal(&wal_path, 0, "base",
            arc_full.path(), ArchiveOptions { chunk_bytes: 1 << 20 }).unwrap();

        // Phase 2: write 50..100 and take an incremental
        // archive from `split_offset`.
        let final_lsn;
        {
            let store = RowStore::open(src.path()).unwrap();
            write_rows(&store, 50..100);
            final_lsn = store.last_lsn();
        }
        let arc_inc = tempfile::tempdir().unwrap();
        let (inc_manifest, inc_receipt) = write_from_wal(
            &wal_path, split_offset, "inc",
            arc_inc.path(), ArchiveOptions { chunk_bytes: 1 << 20 },
        ).unwrap();
        assert_eq!(inc_manifest.from_wal_offset, split_offset);
        assert!(inc_receipt.to_wal_offset > split_offset,
            "incremental archive should carry new bytes");

        // Restore base, then apply incremental on top. The
        // reopened store must show rows 0..100.
        let dst = tempfile::tempdir().unwrap();
        let target_wal = dst.path().join("rows.wal");
        apply_to_wal(arc_full.path(), &target_wal).unwrap();
        apply_to_wal(arc_inc.path(),  &target_wal).unwrap();
        let dst_store = RowStore::open(dst.path()).unwrap();
        assert_eq!(dst_store.last_lsn(), final_lsn);
        for i in 0..100u64 {
            assert_eq!(dst_store.get(&i.to_be_bytes()),
                Some(format!("v{i}").into_bytes()));
        }
    }

    #[test]
    fn verify_detects_chunk_corruption_before_apply() {
        let src = tempfile::tempdir().unwrap();
        let src_store = RowStore::open(src.path()).unwrap();
        write_rows(&src_store, 0..20);
        drop(src_store);
        let archive = tempfile::tempdir().unwrap();
        write_from_wal(&src.path().join("rows.wal"), 0, "c",
            archive.path(), ArchiveOptions { chunk_bytes: 256 }).unwrap();
        // Corrupt chunk 0 by flipping one byte.
        let chunk0 = archive.path().join("chunk-00000.bin");
        let mut bytes = std::fs::read(&chunk0).unwrap();
        bytes[0] ^= 0xff;
        std::fs::write(&chunk0, &bytes).unwrap();
        let err = verify(archive.path()).unwrap_err();
        assert!(matches!(err, ArchiveError::ChunkCrc { .. }),
            "expected ChunkCrc, got {err:?}");
        // apply_to_wal MUST refuse before writing anything.
        let dst = tempfile::tempdir().unwrap();
        let target_wal = dst.path().join("rows.wal");
        let err = apply_to_wal(archive.path(), &target_wal).unwrap_err();
        assert!(matches!(err, ArchiveError::ChunkCrc { .. }));
        assert!(!target_wal.exists(),
            "corrupted archive must not have created a target WAL");
    }

    #[test]
    fn apply_rejects_misaligned_target_wal() {
        let src = tempfile::tempdir().unwrap();
        let src_store = RowStore::open(src.path()).unwrap();
        write_rows(&src_store, 0..10);
        drop(src_store);
        let wal_path = src.path().join("rows.wal");
        let split = std::fs::metadata(&wal_path).unwrap().len();

        let arc = tempfile::tempdir().unwrap();
        write_from_wal(&wal_path, split, "inc-only",
            arc.path(), ArchiveOptions::default()).unwrap();
        // The incremental manifest's from_wal_offset == split.
        // Applying onto a brand-new (empty) target WAL must
        // fail with misalignment, not silently create a hole.
        let dst = tempfile::tempdir().unwrap();
        // Write some rows first so there IS a WAL file with
        // unexpected length (not matching `split`).
        let seed = RowStore::open(dst.path()).unwrap();
        seed.put(b"x", b"y").unwrap();
        let seed_wal_len = std::fs::metadata(dst.path().join("rows.wal")).unwrap().len();
        drop(seed);
        assert_ne!(seed_wal_len, split);
        let err = apply_to_wal(arc.path(), &dst.path().join("rows.wal")).unwrap_err();
        assert!(matches!(err, ArchiveError::ApplyMisalignment { .. }),
            "got {err:?}");
    }

    #[test]
    fn write_refuses_non_empty_target_directory() {
        let src = tempfile::tempdir().unwrap();
        RowStore::open(src.path()).unwrap().put(b"k", b"v").unwrap();
        let out = tempfile::tempdir().unwrap();
        std::fs::write(out.path().join("stray"), b"x").unwrap();
        let err = write_from_wal(
            &src.path().join("rows.wal"), 0, "x", out.path(),
            ArchiveOptions::default()).unwrap_err();
        assert!(matches!(err, ArchiveError::TargetNotEmpty(_)));
    }

    #[test]
    fn write_refuses_offset_past_wal_end() {
        let src = tempfile::tempdir().unwrap();
        RowStore::open(src.path()).unwrap().put(b"k", b"v").unwrap();
        let out = tempfile::tempdir().unwrap();
        // tempdir is already empty (except nothing), so we
        // ask for an offset beyond the WAL file.
        let wal_len = std::fs::metadata(src.path().join("rows.wal")).unwrap().len();
        let err = write_from_wal(
            &src.path().join("rows.wal"), wal_len + 1, "x", out.path(),
            ArchiveOptions::default()).unwrap_err();
        assert!(matches!(err, ArchiveError::OffsetPastEnd { .. }));
    }
}
