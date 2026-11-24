//! ADR 0314 — Studio-facing SQL sessions.
//!
//! A `SqlSessionRegistry` issues tickets that bind a
//! principal to a live reference to `bradian_relational::SqlEngine`
//! and track per-session activity for idle-timeout
//! enforcement. A session calls `execute(sql)` on its
//! ticket; the registry sweeps expired sessions out of
//! band via `sweep(now)`.
//!
//! This slice ships the real engine binding and the real
//! idle-expiry sweep. HTTP routes that mint tickets from
//! an authenticated principal live in `routes.rs` and
//! are a dedicated follow-up (they need the full
//! authn/authz plumbing already present in `auth.rs`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use parking_lot::Mutex;
use thiserror::Error;

/// Opaque session identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct SessionId(pub u64);

/// Live session state. `principal` is the authenticated
/// identity; `last_used` is updated on every successful
/// `execute` call and read by the sweeper.
#[derive(Clone, Debug)]
pub struct SqlSession {
    pub id:        SessionId,
    pub principal: String,
    pub opened_at: SystemTime,
    pub last_used: SystemTime,
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("session {0:?} not found or already closed")]
    NotFound(SessionId),
    #[error("session {0:?} is expired (idle beyond timeout)")]
    Expired(SessionId),
    #[error(transparent)]
    Sql(#[from] SqlError),
}
pub type SessionResult<T> = Result<T, SessionError>;

/// Shared SQL session registry. Holds an `Arc<SqlEngine>`
/// so every session executes against the same engine. The
/// registry itself is cheap to clone (`Arc` under the
/// hood) so it can be stored in axum state.
#[derive(Clone)]
pub struct SqlSessionRegistry {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for SqlSessionRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlSessionRegistry")
         .field("idle_timeout", &self.inner.idle_timeout)
         .field("sessions",     &self.inner.sessions.lock().len())
         .finish()
    }
}

struct Inner {
    engine:        Arc<SqlEngine>,
    idle_timeout:  Duration,
    next_id:       AtomicU64,
    sessions:      Mutex<HashMap<SessionId, SqlSession>>,
}

impl SqlSessionRegistry {
    pub fn new(engine: Arc<SqlEngine>, idle_timeout: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                engine, idle_timeout,
                next_id:  AtomicU64::new(0),
                sessions: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn idle_timeout(&self) -> Duration { self.inner.idle_timeout }

    /// Open a new session for the given principal. Current
    /// wall-clock is supplied by the caller so tests can
    /// drive the clock deterministically.
    pub fn open(&self, principal: impl Into<String>, now: SystemTime) -> SqlSession {
        let id = SessionId(self.inner.next_id.fetch_add(1, Ordering::SeqCst) + 1);
        let s  = SqlSession {
            id, principal: principal.into(),
            opened_at: now, last_used: now,
        };
        self.inner.sessions.lock().insert(id, s.clone());
        s
    }

    pub fn get(&self, id: SessionId) -> Option<SqlSession> {
        self.inner.sessions.lock().get(&id).cloned()
    }

    /// Executes `sql` against the bound `SqlEngine` on
    /// behalf of `id`. Refuses if the session is missing
    /// or has gone idle past the configured timeout
    /// (relative to the supplied `now`). On success,
    /// `last_used` advances to `now`.
    pub fn execute(
        &self, id: SessionId, sql: &str, now: SystemTime,
    ) -> SessionResult<SqlResultSet> {
        {
            let mut g = self.inner.sessions.lock();
            let s = g.get_mut(&id).ok_or(SessionError::NotFound(id))?;
            if is_expired(s, now, self.inner.idle_timeout) {
                g.remove(&id);
                return Err(SessionError::Expired(id));
            }
            s.last_used = now;
        }
        Ok(self.inner.engine.execute(sql)?)
    }

    /// Explicit close. Returns true if the session existed.
    pub fn close(&self, id: SessionId) -> bool {
        self.inner.sessions.lock().remove(&id).is_some()
    }

    /// Sweep expired sessions. Returns the IDs that were
    /// evicted so a caller can audit the sweep.
    pub fn sweep(&self, now: SystemTime) -> Vec<SessionId> {
        let mut evicted = Vec::new();
        let mut g = self.inner.sessions.lock();
        g.retain(|id, s| {
            if is_expired(s, now, self.inner.idle_timeout) {
                evicted.push(*id); false
            } else { true }
        });
        evicted
    }

    pub fn len(&self) -> usize { self.inner.sessions.lock().len() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}

fn is_expired(s: &SqlSession, now: SystemTime, timeout: Duration) -> bool {
    now.duration_since(s.last_used).map(|d| d > timeout).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fresh_registry(timeout: Duration) -> (tempfile::TempDir, SqlSessionRegistry) {
        let dir = tempdir().unwrap();
        let engine = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, value BIGINT, PRIMARY KEY(id))",
            "INSERT INTO t.public.items(id, value) VALUES(1, 100)",
            "INSERT INTO t.public.items(id, value) VALUES(2, 200)",
        ] { engine.execute(sql).unwrap(); }
        (dir, SqlSessionRegistry::new(Arc::new(engine), timeout))
    }

    #[test]
    fn open_issues_unique_ids_and_execute_runs_real_sql() {
        let (_dir, r) = fresh_registry(Duration::from_secs(30));
        let t0 = SystemTime::UNIX_EPOCH;
        let a = r.open("alice", t0);
        let b = r.open("bob", t0);
        assert_ne!(a.id, b.id);
        let rs = r.execute(a.id,
            "SELECT value FROM t.public.items WHERE id = 1",
            t0 + Duration::from_secs(1)).unwrap();
        match rs {
            SqlResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 1);
            }
            other => panic!("expected rows, got {other:?}"),
        }
    }

    #[test]
    fn idle_session_expires_on_next_execute() {
        let (_dir, r) = fresh_registry(Duration::from_secs(10));
        let t0 = SystemTime::UNIX_EPOCH;
        let s = r.open("carol", t0);
        // Just over the timeout — execute is refused with
        // Expired and the session is evicted.
        let now = t0 + Duration::from_secs(11);
        let err = r.execute(s.id, "SELECT 1", now).unwrap_err();
        assert!(matches!(err, SessionError::Expired(id) if id == s.id));
        assert!(r.get(s.id).is_none(),
            "expired session must have been evicted on access");
    }

    #[test]
    fn execute_advances_last_used_so_timeout_resets() {
        let (_dir, r) = fresh_registry(Duration::from_secs(10));
        let t0 = SystemTime::UNIX_EPOCH;
        let s = r.open("dan", t0);
        // Nine seconds in — within window.
        r.execute(s.id, "SELECT id FROM t.public.items WHERE id = 1",
            t0 + Duration::from_secs(9)).unwrap();
        // Nine seconds AFTER last_used — still in window.
        r.execute(s.id, "SELECT id FROM t.public.items WHERE id = 2",
            t0 + Duration::from_secs(9 + 9)).unwrap();
        assert!(r.get(s.id).is_some());
    }

    #[test]
    fn sweep_evicts_only_idle_sessions_and_reports_ids() {
        let (_dir, r) = fresh_registry(Duration::from_secs(5));
        let t0 = SystemTime::UNIX_EPOCH;
        let a = r.open("a", t0);
        let b = r.open("b", t0);
        // Refresh `b` right before the sweep.
        r.execute(b.id,
            "SELECT id FROM t.public.items WHERE id = 1",
            t0 + Duration::from_secs(4)).unwrap();
        let evicted = r.sweep(t0 + Duration::from_secs(6));
        assert_eq!(evicted, vec![a.id]);
        assert!(r.get(a.id).is_none());
        assert!(r.get(b.id).is_some());
    }

    #[test]
    fn execute_unknown_or_closed_session_is_rejected() {
        let (_dir, r) = fresh_registry(Duration::from_secs(10));
        let t0 = SystemTime::UNIX_EPOCH;
        let s = r.open("eve", t0);
        r.close(s.id);
        let err = r.execute(s.id, "SELECT 1", t0).unwrap_err();
        assert!(matches!(err, SessionError::NotFound(id) if id == s.id));
        let bogus = SessionId(99_999);
        let err = r.execute(bogus, "SELECT 1", t0).unwrap_err();
        assert!(matches!(err, SessionError::NotFound(id) if id == bogus));
    }

    #[test]
    fn sql_errors_from_the_engine_are_surfaced_verbatim() {
        let (_dir, r) = fresh_registry(Duration::from_secs(60));
        let t0 = SystemTime::UNIX_EPOCH;
        let s = r.open("f", t0);
        let err = r.execute(s.id, "NOT VALID SQL", t0).unwrap_err();
        match err {
            SessionError::Sql(_) => {}
            other => panic!("expected engine error to surface, got {other:?}"),
        }
        // Session is still alive after an engine error.
        assert!(r.get(s.id).is_some());
    }

    #[test]
    fn registry_is_shareable_across_clones_backed_by_arc() {
        let (_dir, r) = fresh_registry(Duration::from_secs(30));
        let t0 = SystemTime::UNIX_EPOCH;
        let a_clone = r.clone();
        let session = a_clone.open("g", t0);
        // Opened via clone, visible via original.
        assert!(r.get(session.id).is_some());
        r.close(session.id);
        assert!(a_clone.get(session.id).is_none());
    }
}
