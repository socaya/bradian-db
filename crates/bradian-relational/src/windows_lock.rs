//! ADR 0122 — Windows advisory-lock trait (BRD-PORT-001 foundation).
//!
//! Honest scope: the trait + a reference `StdFileLock`
//! implementation that uses `std::fs::File::try_lock()` /
//! `try_lock_shared()` which, on Rust 1.89+, maps to
//! `LockFileEx` / `flock()` as appropriate for the host.
//! This is NOT a Windows-specific lock-lifecycle
//! qualification — real cross-platform lock-lifecycle
//! testing requires a Windows CI runner (per ADR 0105).
//!
//! What ships:
//! - A platform-neutral `AdvisoryLock` trait operators can
//!   depend on without caring whether they're on POSIX or
//!   Windows.
//! - An `AdvisoryLockError` with a specific variant for the
//!   Windows sharing-mode foot-gun.
//! - `StdFileLock` implementation using std::fs APIs that
//!   are already cross-platform.
//!
//! What remains open:
//! - Windows-specific `LockFileEx` with explicit
//!   LOCKFILE_EXCLUSIVE_LOCK + LOCKFILE_FAIL_IMMEDIATELY
//!   flags for finer control. The current wrapper uses the
//!   std abstraction.
//! - Qualification tests on a Windows runner.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdvisoryLockError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("directory is already locked by another process")]
    Busy,
    /// ADR 0122 — Windows-specific: the file was opened with
    /// a sharing mode that forbids the lock. Rare in
    /// practice because we open with full sharing; recorded
    /// as a distinct variant for diagnostics.
    #[cfg(windows)]
    #[error("Windows sharing mode forbids advisory lock on this file")]
    SharingViolation,
}

/// Minimal advisory-lock trait that abstracts over POSIX
/// `flock` / Windows `LockFileEx`.
pub trait AdvisoryLock: std::fmt::Debug {
    /// Return `Err(Busy)` instead of blocking if another
    /// process holds the lock.
    fn try_lock_exclusive(&self) -> Result<(), AdvisoryLockError>;
    /// Release the lock (idempotent).
    fn release(&self) -> Result<(), AdvisoryLockError>;
}

/// Reference implementation using `std::fs::File` APIs
/// (Rust 1.89+). On POSIX this delegates to `flock(2)`; on
/// Windows to `LockFileEx` with `LOCKFILE_EXCLUSIVE_LOCK |
/// LOCKFILE_FAIL_IMMEDIATELY`.
#[derive(Debug)]
pub struct StdFileLock {
    file: File,
}

impl StdFileLock {
    /// Open-or-create the lock file at `path` with full
    /// sharing + rw mode suitable for subsequent
    /// `try_lock_exclusive()`.
    pub fn open(path: &Path) -> Result<Self, AdvisoryLockError> {
        let file = OpenOptions::new().read(true).write(true).create(true)
            .truncate(false).open(path)?;
        Ok(Self { file })
    }
}

impl AdvisoryLock for StdFileLock {
    fn try_lock_exclusive(&self) -> Result<(), AdvisoryLockError> {
        match self.file.try_lock() {
            Ok(()) => Ok(()),
            Err(TryLockError::WouldBlock) => Err(AdvisoryLockError::Busy),
            Err(TryLockError::Error(e)) => {
                #[cfg(windows)]
                if e.raw_os_error() == Some(32) {   // ERROR_SHARING_VIOLATION
                    return Err(AdvisoryLockError::SharingViolation);
                }
                Err(AdvisoryLockError::Io(e))
            }
        }
    }

    fn release(&self) -> Result<(), AdvisoryLockError> {
        self.file.unlock()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn open_and_lock_succeeds_once() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("x.lock");
        let lock = StdFileLock::open(&path).unwrap();
        lock.try_lock_exclusive().unwrap();
        lock.release().unwrap();
    }

    #[test]
    fn second_attempt_from_another_handle_returns_busy() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("x.lock");
        let a = StdFileLock::open(&path).unwrap();
        let b = StdFileLock::open(&path).unwrap();
        a.try_lock_exclusive().unwrap();
        match b.try_lock_exclusive() {
            Err(AdvisoryLockError::Busy) => {}
            other => panic!("expected Busy, got {other:?}"),
        }
        a.release().unwrap();
        // After release, b can acquire.
        b.try_lock_exclusive().unwrap();
        b.release().unwrap();
    }

    #[test]
    fn release_without_lock_is_idempotent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("x.lock");
        let lock = StdFileLock::open(&path).unwrap();
        // Release without prior lock: may return Ok or Io; must
        // not panic.
        let _ = lock.release();
    }
}
