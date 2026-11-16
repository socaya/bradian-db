//! ADR 0114 — operation-lock timeout policy.
//!
//! Single-process BRD-TXN-003 primitive. `parking_lot::Mutex`
//! has `try_lock_for(duration)` which returns a timeout
//! option. This module wraps it in a typed guard so engines
//! can opt-in to bounded waits.
//!
//! Honest scope: single-process only. Multi-process / multi-
//! node lock coordination remains blocked on §13 consensus
//! decisions.

use std::time::Duration;

use parking_lot::Mutex;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LockError {
    #[error("operation lock timed out after {ms}ms")]
    Timeout { ms: u64 },
}

/// Policy for acquiring the per-engine operation mutex.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LockPolicy {
    /// Pre-ADR-0114 behaviour. Block indefinitely.
    #[default]
    Blocking,
    /// Return `LockError::Timeout` after `duration`.
    Timeout(Duration),
}

/// Acquire the mutex under the given policy. Returns the
/// guard on success or `Timeout` on policy breach.
pub fn acquire<'m>(mutex: &'m Mutex<()>, policy: LockPolicy)
    -> Result<parking_lot::MutexGuard<'m, ()>, LockError>
{
    match policy {
        LockPolicy::Blocking => Ok(mutex.lock()),
        LockPolicy::Timeout(d) => mutex.try_lock_for(d).ok_or(LockError::Timeout {
            ms: d.as_millis() as u64,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn blocking_acquires_without_timeout() {
        let m = Mutex::new(());
        let g = acquire(&m, LockPolicy::Blocking).unwrap();
        drop(g);
    }

    #[test]
    fn timeout_fires_when_mutex_held() {
        let m = Arc::new(Mutex::new(()));
        let g = m.lock();   // held
        let m2 = m.clone();
        let handle = thread::spawn(move || {
            acquire(&m2, LockPolicy::Timeout(Duration::from_millis(50)))
                .map(|_| ()).err()
        });
        let err = handle.join().unwrap().expect("expected timeout");
        match err {
            LockError::Timeout { ms } => assert!(ms >= 50),
        }
        drop(g);
    }

    #[test]
    fn timeout_succeeds_when_mutex_free() {
        let m = Mutex::new(());
        let g = acquire(&m, LockPolicy::Timeout(Duration::from_millis(50))).unwrap();
        drop(g);
    }
}
