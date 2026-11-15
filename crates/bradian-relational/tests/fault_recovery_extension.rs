//! G1-FAULTS matrix extension — three scenarios flagged by the
//! comprehensive gaps analysis §9:
//!   1. Parallel-open race under the existing lock.
//!   2. WAL header-only (torn write between header and body).
//!   3. Lock-file deletion mid-session.
//!
//! These exercise the real recovery path on real files and document
//! honest boundaries (lock-file deletion breaks the single-writer
//! invariant on POSIX — this is an operator foot-gun, not a
//! guarantee).

use std::fs::{self, OpenOptions};
use std::io::Write;

use bradian_relational::row_store::{RowStore, StoreError};
use tempfile::tempdir;

// ---------- §9 item: parallel-open race ----------

#[test]
fn parallel_open_under_existing_lock_second_returns_busy() {
    let dir = tempdir().unwrap();
    let first = RowStore::open(dir.path()).unwrap();
    first.put(b"k", b"v").unwrap();
    // Second open on the same directory must refuse with Busy.
    match RowStore::open(dir.path()) {
        Err(StoreError::Busy) => {}
        other => panic!("expected StoreError::Busy on concurrent open; got {other:?}"),
    }
    // First handle is still usable.
    first.put(b"k2", b"v2").unwrap();
    assert_eq!(first.get(b"k"), Some(b"v".to_vec()));
    assert_eq!(first.get(b"k2"), Some(b"v2".to_vec()));
    drop(first);
    // After drop, the lock is released; a fresh open succeeds and sees both rows.
    let reopened = RowStore::open(dir.path()).unwrap();
    assert_eq!(reopened.get(b"k"), Some(b"v".to_vec()));
    assert_eq!(reopened.get(b"k2"), Some(b"v2".to_vec()));
}

#[test]
fn three_parallel_opens_second_and_third_both_fail_with_busy() {
    // Stronger: with one lock-holder, every subsequent open must fail.
    let dir = tempdir().unwrap();
    let first = RowStore::open(dir.path()).unwrap();
    first.put(b"a", b"1").unwrap();
    assert!(matches!(RowStore::open(dir.path()), Err(StoreError::Busy)));
    assert!(matches!(RowStore::open(dir.path()), Err(StoreError::Busy)));
    // First is still healthy after the two refused opens.
    first.put(b"b", b"2").unwrap();
    assert_eq!(first.get(b"a"), Some(b"1".to_vec()));
    assert_eq!(first.get(b"b"), Some(b"2".to_vec()));
}

// ---------- §9 item: WAL header-only (torn write between header and body) ----------

#[test]
fn wal_header_only_no_body_recovers_at_last_complete_frame() {
    // Outcome: writer crashes after fsync'ing the 12-byte WAL frame
    // header but before writing any body bytes. The replay's body
    // read must hit UnexpectedEof and treat the frame as torn
    // (returning Ok at the last complete frame), not corruption.
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"alpha", b"1").unwrap();
        store.put(b"beta", b"2").unwrap();
        // snapshot the valid-WAL end state.
    }
    let wal = dir.path().join("rows.wal");
    let valid_bytes = fs::read(&wal).unwrap();
    let valid_len = valid_bytes.len();
    // Craft a valid header that declares a 32-byte body, but DO NOT
    // write any body bytes. Magic = b"BRWL"; body_len = 32 (fits the
    // 13..=MAX_WAL_BODY range check); CRC arbitrary (will never be
    // checked because the body read fails first).
    let mut torn_header = [0u8; 12];
    torn_header[..4].copy_from_slice(b"BRWL");
    torn_header[4..8].copy_from_slice(&32u32.to_le_bytes());
    torn_header[8..12].copy_from_slice(&0u32.to_le_bytes());
    {
        let mut file = OpenOptions::new().append(true).open(&wal).unwrap();
        file.write_all(&torn_header).unwrap();
        file.sync_all().unwrap();
    }
    // Reopen: header-only torn frame must be treated as end-of-WAL.
    let reopened = RowStore::open(dir.path()).unwrap();
    assert_eq!(reopened.get(b"alpha"), Some(b"1".to_vec()));
    assert_eq!(reopened.get(b"beta"), Some(b"2".to_vec()));
    // Writing a new row after torn-frame recovery must succeed.
    reopened.put(b"gamma", b"3").unwrap();
    assert_eq!(reopened.get(b"gamma"), Some(b"3".to_vec()));
    // Length check: recovery left the torn header on disk (or trimmed it
    // — either is acceptable; we only verify the file is now at least as
    // long as the original valid WAL, since a new frame has been appended).
    let after = fs::metadata(&wal).unwrap().len();
    assert!(after >= valid_len as u64);
}

#[test]
fn wal_header_only_body_truncated_mid_read_recovers() {
    // Variant: header declares body_len=32, but the writer gets through
    // only 10 of those bytes before crashing. The body read hits
    // UnexpectedEof and recovery stops at the last complete frame.
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"committed", b"before-crash").unwrap();
    }
    let wal = dir.path().join("rows.wal");
    let mut torn_header = [0u8; 12];
    torn_header[..4].copy_from_slice(b"BRWL");
    torn_header[4..8].copy_from_slice(&32u32.to_le_bytes());
    torn_header[8..12].copy_from_slice(&0u32.to_le_bytes());
    {
        let mut file = OpenOptions::new().append(true).open(&wal).unwrap();
        file.write_all(&torn_header).unwrap();
        file.write_all(&[0xDEu8; 10]).unwrap();   // 10 of 32 declared body bytes
        file.sync_all().unwrap();
    }
    let reopened = RowStore::open(dir.path()).unwrap();
    assert_eq!(reopened.get(b"committed"), Some(b"before-crash".to_vec()));
}

// ---------- §9 item: lock-file deletion mid-session (honest boundary) ----------

#[cfg(unix)]
#[test]
fn lock_file_deletion_mid_session_breaks_single_writer_invariant_honest_boundary() {
    // POSIX advisory locks (flock/fcntl) are tied to the file DESCRIPTOR
    // the lock-holder opened, not to the pathname. If an operator
    // manually deletes `rows.lock` while a store is open:
    //   - The first handle still holds its advisory lock on the now-
    //     unlinked inode. Its writes remain safe.
    //   - A subsequent open() creates a NEW `rows.lock` inode and takes
    //     the lock on THAT inode. The kernel sees no conflict.
    //
    // The result is TWO concurrent writers on the same data directory.
    // This is an operator foot-gun, not a guarantee the kernel can
    // provide. This test LOCKS THE BEHAVIOR IN HONESTLY so the
    // documentation matches reality.
    let dir = tempdir().unwrap();
    let first = RowStore::open(dir.path()).unwrap();
    first.put(b"first-writer", b"owns-the-lock").unwrap();

    // Operator deletes the lock file while the first handle is still live.
    fs::remove_file(dir.path().join("rows.lock")).unwrap();

    // Second open succeeds on POSIX — the honest boundary.
    let second_result = RowStore::open(dir.path());
    assert!(second_result.is_ok(),
        "POSIX advisory-lock semantics make lock-file deletion a foot-gun; \
         if this ever starts returning Busy, update the docs to reflect the \
         stronger guarantee");

    // Both handles are live. The second one can read the committed state
    // and even write to it, violating single-writer. This is an operator
    // mistake, not a corruption.
    let second = second_result.unwrap();
    assert_eq!(second.get(b"first-writer"), Some(b"owns-the-lock".to_vec()));

    drop(first);
    drop(second);
}
