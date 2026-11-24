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
