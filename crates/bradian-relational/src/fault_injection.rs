//! ADR 0252 — byte-level fault injection utilities for tests.
//!
//! These functions directly damage files on disk so the test
//! harness can exercise the recovery paths of existing stores
//! (BRWL WAL, BRPG checkpoints, user journals). They are
//! deliberately public so integration tests across many
//! crates can share them without duplicating the unsafe
//! boundary code.
//!
//! **Not for production use.** These functions corrupt data
//! on purpose; the only intended callers are test harnesses
//! and investigation tooling. The module is namespaced under
//! `fault_injection` so production audits can grep for its
//! usage and flag any non-test call site.

use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

/// ADR 0288 — injectable fault policy. The trait is the
/// contract a future storage-layer shim will consult before
/// every `write` / `sync_data` call. Current test code can
/// poll it manually in scripted workloads via ADR 0289.
pub trait FaultPolicy: Send + Sync {
    /// Called before a logical write. Returning `Err`
    /// simulates a storage write failure.
    fn on_write(&self) -> io::Result<()> { Ok(()) }
    /// Called before a logical sync. Returning `Err`
    /// simulates an fsync failure (durability lost).
    fn on_sync(&self) -> io::Result<()> { Ok(()) }
}

/// Default policy — nothing fails.
#[derive(Clone, Copy, Debug, Default)]
pub struct NeverFail;
impl FaultPolicy for NeverFail {}

/// ADR 0288 — fail the Nth write. After the fault fires,
/// subsequent writes succeed (one-shot).
pub struct FailAfterNWrites {
    remaining: AtomicUsize,
    fired: std::sync::atomic::AtomicBool,
}
impl FailAfterNWrites {
    pub fn new(n: usize) -> Self {
        Self { remaining: AtomicUsize::new(n),
               fired: std::sync::atomic::AtomicBool::new(false) }
    }
    pub fn has_fired(&self) -> bool {
        self.fired.load(Ordering::SeqCst)
    }
}
impl FaultPolicy for FailAfterNWrites {
    fn on_write(&self) -> io::Result<()> {
        if self.fired.load(Ordering::SeqCst) { return Ok(()); }
        let left = self.remaining.fetch_update(
            Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_sub(1));
        match left {
            Ok(1) => {
                self.fired.store(true, Ordering::SeqCst);
                Err(io::Error::new(io::ErrorKind::Other, "simulated write fault"))
            }
            _ => Ok(()),
        }
    }
}

/// ADR 0288 — simulate an fsync that silently loses
/// durability. Returns Ok but marks the fire so test code
/// knows to treat the store as not-durably-synced.
pub struct DropOnNthSync {
    remaining: AtomicUsize,
    fired: std::sync::atomic::AtomicBool,
}
impl DropOnNthSync {
    pub fn new(n: usize) -> Self {
        Self { remaining: AtomicUsize::new(n),
               fired: std::sync::atomic::AtomicBool::new(false) }
    }
    pub fn has_fired(&self) -> bool {
        self.fired.load(Ordering::SeqCst)
    }
}
impl FaultPolicy for DropOnNthSync {
    fn on_sync(&self) -> io::Result<()> {
        if self.fired.load(Ordering::SeqCst) { return Ok(()); }
        let left = self.remaining.fetch_update(
            Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_sub(1));
        if matches!(left, Ok(1)) {
            self.fired.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

/// Return the length of a file in bytes. Thin wrapper around
/// `fs::metadata().len()`, exposed so callers can compute
/// relative offsets without pulling in `std::fs` directly.
pub fn file_len(path: &Path) -> io::Result<u64> {
    std::fs::metadata(path).map(|m| m.len())
}

/// Truncate a file to exactly `new_len` bytes. Fails with
/// `InvalidInput` if `new_len` exceeds the current length —
/// this helper never grows files. Growing requires explicit
/// zero-write and is a separate concern.
pub fn truncate_file_to(path: &Path, new_len: u64) -> io::Result<()> {
    let current = file_len(path)?;
    if new_len > current {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("truncate_file_to: new_len {new_len} > current {current}"),
        ));
    }
    let file = OpenOptions::new().write(true).open(path)?;
    file.set_len(new_len)?;
    file.sync_all()?;
    Ok(())
}

/// Flip (XOR) one byte at `offset` in the file. Useful for
/// simulating single-bit storage corruption. Fails if the
/// offset is past end-of-file.
pub fn flip_byte_at(path: &Path, offset: u64) -> io::Result<()> {
    let len = file_len(path)?;
    if offset >= len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("flip_byte_at: offset {offset} >= file length {len}"),
        ));
    }
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = [0u8; 1];
    file.read_exact(&mut buf)?;
    buf[0] ^= 0xFF;
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(&buf)?;
    file.sync_all()?;
    Ok(())
}

/// Zero a contiguous range of `len` bytes starting at
/// `offset`. Fails if the range extends past end-of-file.
pub fn zero_range(path: &Path, offset: u64, len: u64) -> io::Result<()> {
    let total = file_len(path)?;
    if offset.saturating_add(len) > total {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("zero_range: {offset}+{len} > file length {total}"),
        ));
    }
    let mut file = OpenOptions::new().write(true).open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let chunk = vec![0u8; len.min(4096) as usize];
    let mut remaining = len;
    while remaining > 0 {
        let take = remaining.min(chunk.len() as u64) as usize;
        file.write_all(&chunk[..take])?;
        remaining -= take as u64;
    }
    file.sync_all()?;
    Ok(())
}
