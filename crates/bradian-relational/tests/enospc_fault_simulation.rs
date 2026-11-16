//! ADR 0111 — ENOSPC / EFBIG fault harness (POSIX only).
//!
//! Uses `setrlimit(RLIMIT_FSIZE, ...)` to synthesize a small
//! file-size cap in the current process, exercises RowStore
//! against it, and verifies that Bradian propagates the
//! resulting `std::io::Error` through `StoreError::Io` without
//! corrupting the data files.
//!
//! Honest scope: this simulates the write-returns-errno-code
//! part of ENOSPC. It does NOT reproduce:
//!   - mid-fsync power loss across a real disk-full event;
//!   - partial fsync + torn write at the hardware boundary;
//!   - cross-process RLIMIT isolation (the limit is per-
//!     process on POSIX — run `cargo test` single-threaded
//!     or the serial mutex below).
//!
//! Non-POSIX: this test is `#[cfg(unix)]`.

#![cfg(unix)]

use std::sync::Mutex;

use bradian_relational::row_store::{RowStore, StoreError};
use tempfile::tempdir;

// All tests in this file touch the process-wide RLIMIT_FSIZE
// and the SIGXFSZ disposition. Serialise so parallel execution
// does not cross-contaminate.
static SERIAL: Mutex<()> = Mutex::new(());

/// Install an ignore handler for SIGXFSZ. Without this, the
/// kernel's default action for exceeding RLIMIT_FSIZE is to
/// terminate the process. With the ignore handler, writes
/// that exceed the limit return EFBIG via `-1 / errno`.
///
/// This is idempotent and intentionally not restored — the
/// test process is throwaway and SIGXFSZ ignore is safe.
fn ignore_sigxfsz_once() {
    use std::sync::Once;
    static DONE: Once = Once::new();
    DONE.call_once(|| {
        #[allow(unsafe_code)]
        // SAFETY: signal() with SIG_IGN on a defined signal
        // number is a thread-safe POSIX syscall that returns
        // the previous handler. We ignore the return value.
        unsafe { libc::signal(libc::SIGXFSZ, libc::SIG_IGN); }
    });
}

/// Save + restore RLIMIT_FSIZE around the test closure.
fn with_fsize_cap<F: FnOnce()>(cap_bytes: u64, f: F) {
    ignore_sigxfsz_once();
    #[allow(unsafe_code)]
    unsafe {
        // SAFETY: rlimit syscalls take a &rlimit struct we own.
        // Soft+hard both set; the test restores on drop.
        let mut original = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        if libc::getrlimit(libc::RLIMIT_FSIZE, &mut original) != 0 {
            panic!("getrlimit failed");
        }
        // Only lower the soft limit; leave the hard limit alone
        // so restore (just raising the soft back) is permitted
        // by the kernel for non-privileged processes.
        let tight = libc::rlimit { rlim_cur: cap_bytes, rlim_max: original.rlim_max };
        if libc::setrlimit(libc::RLIMIT_FSIZE, &tight) != 0 {
            panic!("setrlimit tight failed");
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        // Restore unconditionally even if the test panicked.
        if libc::setrlimit(libc::RLIMIT_FSIZE, &original) != 0 {
            panic!("setrlimit restore failed");
        }
        if let Err(e) = result { std::panic::resume_unwind(e); }
    }
}

// ---------- scenarios ----------

#[test]
fn rlimit_fsize_during_put_propagates_as_io_error_not_corruption() {
    let _g = SERIAL.lock().unwrap();
    let dir = tempdir().unwrap();
    // Open BEFORE tightening the limit so the empty files are
    // created without hitting it.
    let store = RowStore::open(dir.path()).unwrap();
    // Verify the baseline works.
    store.put(b"pre-cap", b"value").unwrap();

    // 512 bytes is aggressive — a put is ~100 bytes of WAL
    // frame, so roughly 4 puts fill the cap.
    with_fsize_cap(512, || {
        // Force many writes to exceed the cap.
        let mut io_errors = 0;
        for i in 0..100 {
            let key = format!("key-{i}");
            let value = vec![b'x'; 128];
            match store.put(key.as_bytes(), &value) {
                Ok(_) => {}
                Err(StoreError::Io(_)) => { io_errors += 1; break; }
                Err(StoreError::Corruption(msg)) => {
                    panic!("ENOSPC/EFBIG must NOT be reported as Corruption: {msg}");
                }
                Err(StoreError::Poisoned) => { io_errors += 1; break; }
                Err(other) => panic!("unexpected error class: {other:?}"),
            }
        }
        assert!(io_errors > 0, "expected at least one Io or Poisoned error when hitting EFBIG");
    });
}

#[test]
fn files_remain_openable_after_cap_is_relaxed() {
    let _g = SERIAL.lock().unwrap();
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"before-cap", b"important").unwrap();

        // Run until the cap error fires. Don't care which specific
        // error propagates — we just need SOME write to fail.
        with_fsize_cap(2048, || {
            for i in 0..1000 {
                let k = format!("k{i}");
                if store.put(k.as_bytes(), &vec![b'x'; 256]).is_err() { break; }
            }
        });
        // The store handle may be poisoned after an Io error on
        // a WAL append. Drop it so the fresh reopen path runs.
    }

    // After cap relaxed and store dropped: reopen must succeed
    // and read the pre-cap data. The post-cap writes may or
    // may not have been recorded; recovery either accepts or
    // trims the torn frame per existing fault-recovery rules.
    let store = RowStore::open(dir.path()).unwrap();
    assert_eq!(
        store.get(b"before-cap"),
        Some(b"important".to_vec()),
        "pre-cap commit must survive the EFBIG event",
    );
    // And new writes under relaxed limits succeed.
    store.put(b"after-recovery", b"ok").unwrap();
}

#[test]
fn rlimit_zero_still_allows_lock_file_but_blocks_data_writes() {
    let _g = SERIAL.lock().unwrap();
    // RLIMIT_FSIZE=0 means every write-grow fails. Opening the
    // store creates the lock + WAL files but their initial
    // creation with zero bytes is allowed; the first real
    // append fails.
    let dir = tempdir().unwrap();
    // Pre-create the files at zero-length by opening outside the cap.
    let store_before = RowStore::open(dir.path()).unwrap();
    drop(store_before);
    // Now tighten the cap and reopen (which may replay WAL - zero
    // bytes so no read-write beyond zero). Any put will fail.
    with_fsize_cap(0, || {
        if let Ok(store) = RowStore::open(dir.path()) {
            let outcome = store.put(b"k", b"v");
            // Must be a non-Corruption error (Io or Poisoned).
            assert!(matches!(outcome, Err(StoreError::Io(_)) | Err(StoreError::Poisoned)),
                "zero FSIZE + put: expected Io or Poisoned, got {outcome:?}");
        }
    });
}
