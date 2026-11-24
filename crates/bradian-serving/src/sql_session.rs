//! ADR 0314 — Studio SQL session **skeleton**.
//!
//! In-memory session book-keeping only. No HTTP routes
//! wired, no idle expiry timer, no SQL statement execution
//! bound to the session.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use parking_lot::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct SessionId(pub u64);

#[derive(Clone, Debug)]
pub struct SessionTicket {
    pub id:        SessionId,
    pub principal: String,
    pub opened_at: SystemTime,
}

#[derive(Debug, Default)]
pub struct SessionRegistry {
    next:     AtomicU64,
    sessions: Mutex<HashMap<SessionId, SessionTicket>>,
}

impl SessionRegistry {
    pub fn new() -> Self { Self::default() }

    pub fn open(&self, principal: impl Into<String>) -> SessionTicket {
        let id = SessionId(self.next.fetch_add(1, Ordering::SeqCst) + 1);
        let t = SessionTicket {
            id, principal: principal.into(), opened_at: SystemTime::now(),
        };
        self.sessions.lock().insert(id, t.clone());
        t
    }

    pub fn get(&self, id: SessionId) -> Option<SessionTicket> {
        self.sessions.lock().get(&id).cloned()
    }

    pub fn close(&self, id: SessionId) -> bool {
        self.sessions.lock().remove(&id).is_some()
    }

    pub fn len(&self) -> usize { self.sessions.lock().len() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_issues_monotonic_ids_and_preserves_principal() {
        let r = SessionRegistry::new();
        let a = r.open("alice");
        let b = r.open("bob");
        assert_eq!(a.principal, "alice");
        assert_eq!(b.principal, "bob");
        assert!(b.id.0 > a.id.0);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn close_removes_and_get_returns_none_after() {
        let r = SessionRegistry::new();
        let t = r.open("carol");
        assert!(r.get(t.id).is_some());
        assert!(r.close(t.id));
        assert!(r.get(t.id).is_none());
        assert!(!r.close(t.id), "second close should return false");
    }
}
