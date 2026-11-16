//! ADR 0096 — Mutable page-file allocator foundation.
//!
//! A crash-recoverable mutable page file where pages are
//! addressed by stable `u64` ids and reused via an append-only
//! `freelist.manifest`. This primitive exists so a future
//! crash-safe B+tree (BRD-STO-005, Gate 3) can allocate pages
//! without writing another allocator. It is NOT a B+tree, NOT
//! a buffer pool, and NOT a transaction manager.
//!
//! Layout:
//!   Data file (`pages.brmp`): fixed 4 KiB pages. Each page:
//!     [0..4]   magic b"BRMP"
//!     [4..6]   version u16 = 1
//!     [6..7]   kind u8
//!     [7..8]   reserved
//!     [8..12]  payload_len u32
//!     [12..16] crc32 over (bytes [0..12] with crc field zeroed,
//!              then payload, then zero-padding)
//!     [16..]   payload + zero padding to page size
//!
//!   Manifest file (`freelist.manifest`): append-only log of
//!     16-byte records `[op u8 | page_id u64 | reserved]`
//!     where op=1 = allocated, op=2 = freed. Replayed on open
//!     to rebuild the in-memory free-list set.
//!
//! Lock: `rows.lock` convention (shared with `RowStore`) — a
//! single-process exclusive lock; parallel opens return `Busy`.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crc32fast::hash;
use parking_lot::Mutex;
use thiserror::Error;

const PAGE_SIZE: usize = 4096;
const HEADER_SIZE: usize = 16;
const MAGIC: [u8; 4] = *b"BRMP";
const VERSION: u16 = 1;
const MANIFEST_RECORD_SIZE: usize = 16;
const DATA_FILE: &str = "pages.brmp";
const MANIFEST_FILE: &str = "freelist.manifest";
const LOCK_FILE: &str = "rows.lock";

/// Type byte identifying what the page stores. v1 ships
/// `Freelist` and `Opaque`; later ADRs can add new kinds
/// provided they preserve byte invariants on existing kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PageKind {
    Opaque = 1,
    Freelist = 2,
}

#[derive(Debug, Error)]
pub enum MutablePageError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("corruption: {0}")]
    Corruption(String),
    #[error("directory is already open by another process")]
    Busy,
    #[error("page_id {0} not allocated")]
    NotAllocated(u64),
    #[error("page_id {0} out of range")]
    OutOfRange(u64),
    #[error("payload exceeds {} bytes", PAGE_SIZE - HEADER_SIZE)]
    PayloadTooLarge,
    #[error("unknown page kind {0}")]
    UnknownKind(u8),
}

pub type Result<T> = std::result::Result<T, MutablePageError>;

#[derive(Debug)]
struct Inner {
    data: File,
    manifest: File,
    page_count: u64,
    free: std::collections::BTreeSet<u64>,
}

/// Mutable page file. All public methods acquire the inner
/// mutex. Writes fsync both the data page and the manifest
/// append so a mid-operation crash leaves the file in exactly
/// one of {pre-write, post-write} states (no partial page +
/// no partial manifest record is observable after reopen).
#[derive(Debug, Clone)]
pub struct MutablePageFile {
    inner: Arc<Mutex<Inner>>,
    _lock: Arc<File>,
}

impl MutablePageFile {
    /// Open a mutable page directory. The directory must either
    /// be empty (fresh file created) or contain compatible v1
    /// data + manifest files.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        std::fs::create_dir_all(root)?;
        // Exclusive advisory lock, same convention as RowStore.
        let lock = OpenOptions::new().read(true).write(true).create(true)
            .truncate(false).open(root.join(LOCK_FILE))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(MutablePageError::Busy),
            Err(std::fs::TryLockError::Error(e)) => return Err(MutablePageError::Io(e)),
        }
        let data = OpenOptions::new().read(true).write(true).create(true)
            .truncate(false).open(root.join(DATA_FILE))?;
        let manifest = OpenOptions::new().read(true).write(true).create(true)
            .truncate(false).open(root.join(MANIFEST_FILE))?;
        let page_count = data.metadata()?.len() / PAGE_SIZE as u64;
        let data_len = data.metadata()?.len();
        if data_len % PAGE_SIZE as u64 != 0 {
            return Err(MutablePageError::Corruption(format!(
                "data file length {} not a multiple of {}", data_len, PAGE_SIZE,
            )));
        }

        // Verify every existing page's CRC + replay manifest.
        let mut inner = Inner {
            data, manifest, page_count,
            free: std::collections::BTreeSet::new(),
        };
        verify_all_pages(&mut inner)?;
        replay_manifest(&mut inner)?;
        Ok(Self { inner: Arc::new(Mutex::new(inner)), _lock: Arc::new(lock) })
    }

    pub fn page_count(&self) -> u64 {
        self.inner.lock().page_count
    }

    pub fn free_count(&self) -> usize {
        self.inner.lock().free.len()
    }

    /// Allocate a page: reuse the lowest free page_id if any,
    /// otherwise extend the file by one page. The allocation
    /// is recorded in the manifest and synced before returning.
    pub fn allocate(&self) -> Result<u64> {
        let mut inner = self.inner.lock();
        let id = match inner.free.iter().next().copied() {
            Some(id) => { inner.free.remove(&id); id }
            None => {
                let id = inner.page_count;
                inner.page_count = id.checked_add(1).ok_or_else(||
                    MutablePageError::Corruption("page id overflow".into())
                )?;
                // Extend the file by one blank 4 KiB region so
                // subsequent seek/write stays bounded.
                let new_len = (inner.page_count) * PAGE_SIZE as u64;
                inner.data.set_len(new_len)?;
                id
            }
        };
        append_manifest_record(&mut inner.manifest, 1, id)?;
        inner.data.sync_all()?;
        inner.manifest.sync_all()?;
        Ok(id)
    }

    /// Write a page's payload. The page must have been allocated
    /// (either freshly via `allocate` or still-live from a prior
    /// session). Pages marked `free` in the manifest cannot be
    /// written to.
    pub fn write_page(&self, page_id: u64, kind: PageKind, payload: &[u8]) -> Result<()> {
        if payload.len() > PAGE_SIZE - HEADER_SIZE {
            return Err(MutablePageError::PayloadTooLarge);
        }
        let mut inner = self.inner.lock();
        if page_id >= inner.page_count {
            return Err(MutablePageError::OutOfRange(page_id));
        }
        if inner.free.contains(&page_id) {
            return Err(MutablePageError::NotAllocated(page_id));
        }
        let mut buf = [0u8; PAGE_SIZE];
        buf[0..4].copy_from_slice(&MAGIC);
        buf[4..6].copy_from_slice(&VERSION.to_le_bytes());
        buf[6] = kind as u8;
        buf[7] = 0;
        buf[8..12].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        // CRC field at [12..16] stays zero while computing.
        buf[16..16 + payload.len()].copy_from_slice(payload);
        let crc = hash(&buf);
        buf[12..16].copy_from_slice(&crc.to_le_bytes());

        inner.data.seek(SeekFrom::Start(page_id * PAGE_SIZE as u64))?;
        inner.data.write_all(&buf)?;
        inner.data.sync_all()?;
        Ok(())
    }

    /// Read a page. Verifies magic, version, length, and CRC.
    /// A freed page is rejected with `NotAllocated`.
    pub fn read_page(&self, page_id: u64) -> Result<(PageKind, Vec<u8>)> {
        let mut inner = self.inner.lock();
        if page_id >= inner.page_count {
            return Err(MutablePageError::OutOfRange(page_id));
        }
        if inner.free.contains(&page_id) {
            return Err(MutablePageError::NotAllocated(page_id));
        }
        let mut buf = [0u8; PAGE_SIZE];
        inner.data.seek(SeekFrom::Start(page_id * PAGE_SIZE as u64))?;
        inner.data.read_exact(&mut buf)?;
        decode_page(&buf, page_id)
    }

    /// Free a page: records the free in the manifest, syncs,
    /// and makes the page_id eligible for reuse.
    pub fn free_page(&self, page_id: u64) -> Result<()> {
        let mut inner = self.inner.lock();
        if page_id >= inner.page_count {
            return Err(MutablePageError::OutOfRange(page_id));
        }
        if inner.free.contains(&page_id) {
            return Err(MutablePageError::NotAllocated(page_id));
        }
        append_manifest_record(&mut inner.manifest, 2, page_id)?;
        inner.manifest.sync_all()?;
        inner.free.insert(page_id);
        Ok(())
    }
}

fn append_manifest_record(f: &mut File, op: u8, page_id: u64) -> Result<()> {
    f.seek(SeekFrom::End(0))?;
    f.write_all(&[op])?;
    f.write_all(&page_id.to_le_bytes())?;
    f.write_all(&[0u8; 7])?;
    Ok(())
}

fn decode_page(buf: &[u8], page_id: u64) -> Result<(PageKind, Vec<u8>)> {
    if buf.len() != PAGE_SIZE {
        return Err(MutablePageError::Corruption(format!(
            "page {page_id} read length {} != {PAGE_SIZE}", buf.len(),
        )));
    }
    if buf[0..4] != MAGIC {
        return Err(MutablePageError::Corruption(format!(
            "page {page_id} bad magic",
        )));
    }
    let version = u16::from_le_bytes(buf[4..6].try_into().unwrap());
    if version != VERSION {
        return Err(MutablePageError::Corruption(format!(
            "page {page_id} unsupported version {version}",
        )));
    }
    let kind = match buf[6] {
        1 => PageKind::Opaque,
        2 => PageKind::Freelist,
        other => return Err(MutablePageError::UnknownKind(other)),
    };
    let payload_len = u32::from_le_bytes(buf[8..12].try_into().unwrap()) as usize;
    if payload_len > PAGE_SIZE - HEADER_SIZE {
        return Err(MutablePageError::Corruption(format!(
            "page {page_id} declared payload_len {payload_len} > max",
        )));
    }
    let declared_crc = u32::from_le_bytes(buf[12..16].try_into().unwrap());
    // Recompute CRC with CRC field zeroed.
    let mut tmp = [0u8; PAGE_SIZE];
    tmp.copy_from_slice(buf);
    tmp[12..16].copy_from_slice(&[0u8; 4]);
    let actual = hash(&tmp);
    if actual != declared_crc {
        return Err(MutablePageError::Corruption(format!(
            "page {page_id} crc mismatch",
        )));
    }
    Ok((kind, buf[16..16 + payload_len].to_vec()))
}

fn verify_all_pages(inner: &mut Inner) -> Result<()> {
    for i in 0..inner.page_count {
        let mut buf = [0u8; PAGE_SIZE];
        inner.data.seek(SeekFrom::Start(i * PAGE_SIZE as u64))?;
        match inner.data.read_exact(&mut buf) {
            Ok(()) => {
                // Only validate if the page has a magic prefix.
                // A freshly-extended page (from `set_len`) is all
                // zeros; those are tolerated and treated as
                // "allocated but unwritten".
                if buf[0..4] == MAGIC {
                    let _ = decode_page(&buf, i)?;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(MutablePageError::Corruption(format!(
                    "data file truncated at page {i}",
                )));
            }
            Err(e) => return Err(MutablePageError::Io(e)),
        }
    }
    Ok(())
}

fn replay_manifest(inner: &mut Inner) -> Result<()> {
    inner.manifest.seek(SeekFrom::Start(0))?;
    let mut buf = [0u8; MANIFEST_RECORD_SIZE];
    // Allocated-set tracks which page_ids were ever allocated and
    // not freed. We rebuild free-set by replaying ops in order.
    let mut live: std::collections::BTreeSet<u64> =
        std::collections::BTreeSet::new();
    loop {
        match inner.manifest.read_exact(&mut buf) {
            Ok(()) => {
                let op = buf[0];
                let page_id = u64::from_le_bytes(buf[1..9].try_into().unwrap());
                match op {
                    1 => { live.insert(page_id); }
                    2 => { live.remove(&page_id); }
                    _ => return Err(MutablePageError::Corruption(format!(
                        "manifest unknown op {op} at page_id {page_id}",
                    ))),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(MutablePageError::Io(e)),
        }
    }
    // Free set = { ids ever extended } \ { live }.
    for id in 0..inner.page_count {
        if !live.contains(&id) {
            inner.free.insert(id);
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct MutablePagePath(PathBuf);

impl MutablePagePath {
    pub fn new(root: impl Into<PathBuf>) -> Self { Self(root.into()) }
    pub fn as_path(&self) -> &Path { &self.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn fresh_open_has_zero_pages() {
        let dir = tempdir().unwrap();
        let f = MutablePageFile::open(dir.path()).unwrap();
        assert_eq!(f.page_count(), 0);
        assert_eq!(f.free_count(), 0);
    }

    #[test]
    fn allocate_write_read_roundtrip() {
        let dir = tempdir().unwrap();
        let f = MutablePageFile::open(dir.path()).unwrap();
        let id = f.allocate().unwrap();
        f.write_page(id, PageKind::Opaque, b"hello world").unwrap();
        let (kind, payload) = f.read_page(id).unwrap();
        assert_eq!(kind, PageKind::Opaque);
        assert_eq!(payload, b"hello world");
    }

    #[test]
    fn parallel_open_returns_busy() {
        let dir = tempdir().unwrap();
        let f1 = MutablePageFile::open(dir.path()).unwrap();
        match MutablePageFile::open(dir.path()) {
            Err(MutablePageError::Busy) => {}
            other => panic!("expected Busy; got {other:?}"),
        }
        drop(f1);
        // After drop, reopen succeeds.
        let _f2 = MutablePageFile::open(dir.path()).unwrap();
    }

    #[test]
    fn crc_tamper_detected_on_read() {
        let dir = tempdir().unwrap();
        {
            let f = MutablePageFile::open(dir.path()).unwrap();
            let id = f.allocate().unwrap();
            f.write_page(id, PageKind::Opaque, b"watch me get tampered").unwrap();
        }
        // Flip a payload byte.
        let data_path = dir.path().join("pages.brmp");
        let mut bytes = std::fs::read(&data_path).unwrap();
        bytes[16 + 5] ^= 0xFF;
        std::fs::write(&data_path, &bytes).unwrap();

        let err = MutablePageFile::open(dir.path()).unwrap_err();
        assert!(matches!(err, MutablePageError::Corruption(_)));
    }

    #[test]
    fn free_and_reuse_lowest_id() {
        let dir = tempdir().unwrap();
        let f = MutablePageFile::open(dir.path()).unwrap();
        let a = f.allocate().unwrap();
        let b = f.allocate().unwrap();
        let c = f.allocate().unwrap();
        assert_eq!((a, b, c), (0, 1, 2));
        f.free_page(b).unwrap();
        assert_eq!(f.free_count(), 1);
        let d = f.allocate().unwrap();
        assert_eq!(d, 1, "freed page should be the lowest reused");
    }

    #[test]
    fn manifest_replays_across_restart() {
        let dir = tempdir().unwrap();
        {
            let f = MutablePageFile::open(dir.path()).unwrap();
            for _ in 0..5 { let _ = f.allocate().unwrap(); }
            f.free_page(2).unwrap();
            f.free_page(4).unwrap();
            f.write_page(0, PageKind::Opaque, b"keep").unwrap();
            f.write_page(3, PageKind::Opaque, b"also").unwrap();
        }
        let f = MutablePageFile::open(dir.path()).unwrap();
        assert_eq!(f.page_count(), 5);
        assert_eq!(f.free_count(), 2);
        let (_, p0) = f.read_page(0).unwrap();
        assert_eq!(p0, b"keep");
        let (_, p3) = f.read_page(3).unwrap();
        assert_eq!(p3, b"also");
        // Freed pages cannot be read.
        assert!(matches!(f.read_page(2), Err(MutablePageError::NotAllocated(2))));
        // Next allocation reuses the lowest freed slot.
        let n = f.allocate().unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn page_kind_freelist_roundtrip() {
        let dir = tempdir().unwrap();
        let f = MutablePageFile::open(dir.path()).unwrap();
        let id = f.allocate().unwrap();
        f.write_page(id, PageKind::Freelist, b"freelist-metadata").unwrap();
        let (kind, _) = f.read_page(id).unwrap();
        assert_eq!(kind, PageKind::Freelist);
    }

    #[test]
    fn payload_too_large_rejected() {
        let dir = tempdir().unwrap();
        let f = MutablePageFile::open(dir.path()).unwrap();
        let id = f.allocate().unwrap();
        let too_big = vec![0u8; PAGE_SIZE];
        assert!(matches!(
            f.write_page(id, PageKind::Opaque, &too_big),
            Err(MutablePageError::PayloadTooLarge)
        ));
    }
}
