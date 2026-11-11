//! Edge-to-server reconciliation (DST-002).
//!
//! The site records every logical change it accepts locally; when the WAN comes back
//! online it replays the log to the server. The server deduplicates by `(change_id,
//! source_version)` and acknowledges the last successfully applied LSN. If the server
//! lags, the client keeps the pending log durable and retries.

use std::sync::Mutex;

use dap_core::{DapError, DapResult};
use serde::{Deserialize, Serialize};

/// Watermark and acknowledgement stamp carried by a site.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteCheckpoint {
    pub site_id: String,
    /// Highest LSN the site believes the server has acknowledged.
    pub acked_lsn: u64,
    /// Highest LSN the site has locally generated.
    pub local_lsn: u64,
    /// UTC ms of the last successful ack round-trip.
    pub last_ack_ms: i64,
}

impl SiteCheckpoint {
    pub fn lag(&self) -> u64 {
        self.local_lsn.saturating_sub(self.acked_lsn)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconciliationOutcome {
    pub applied: u64,
    pub duplicates: u64,
    pub stale: u64,
    pub rejected: u64,
    pub new_acked_lsn: u64,
}

/// In-memory reconciliation log used by tests and by the single-process DAP build. The
/// production server persists this log in the same `.derc` partition infrastructure used
/// for analytical facts.
#[derive(Debug)]
pub struct ReconciliationLog {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    checkpoints: ahash::AHashMap<String, SiteCheckpoint>,
    applied_change_ids: ahash::AHashSet<String>,
}

impl ReconciliationLog {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
        }
    }

    pub fn checkpoint(&self, site_id: &str) -> Option<SiteCheckpoint> {
        self.inner.lock().ok()?.checkpoints.get(site_id).cloned()
    }

    /// Apply a stream of logical changes identified by `(change_id, source_version, lsn)`.
    /// Returns the outcome stats plus the new ack LSN.
    pub fn apply<'a>(
        &self,
        site_id: &str,
        changes: impl IntoIterator<Item = (&'a str, u64, u64)>,
    ) -> DapResult<ReconciliationOutcome> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| DapError::invariant("reconciliation log poisoned"))?;
        inner
            .checkpoints
            .entry(site_id.to_string())
            .or_insert_with(|| SiteCheckpoint {
                site_id: site_id.to_string(),
                acked_lsn: 0,
                local_lsn: 0,
                last_ack_ms: 0,
            });
        let mut applied = 0u64;
        let mut duplicates = 0u64;
        let mut stale = 0u64;
        let mut rejected = 0u64;
        for (change_id, source_version, lsn) in changes {
            let Inner {
                checkpoints,
                applied_change_ids,
            } = &mut *inner;
            let cp = checkpoints.get_mut(site_id).unwrap();
            if applied_change_ids.contains(change_id) {
                duplicates += 1;
                continue;
            }
            if lsn <= cp.acked_lsn {
                stale += 1;
                continue;
            }
            if source_version == 0 {
                rejected += 1;
                continue;
            }
            applied_change_ids.insert(change_id.to_string());
            if lsn > cp.local_lsn {
                cp.local_lsn = lsn;
            }
            cp.acked_lsn = cp.acked_lsn.max(lsn);
            applied += 1;
        }
        let cp = inner.checkpoints.get_mut(site_id).unwrap();
        cp.last_ack_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let acked = cp.acked_lsn;
        Ok(ReconciliationOutcome {
            applied,
            duplicates,
            stale,
            rejected,
            new_acked_lsn: acked,
        })
    }
}

impl Default for ReconciliationLog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idempotent_replay_collapses_duplicates() {
        let log = ReconciliationLog::new();
        let outcome = log
            .apply(
                "siteA",
                vec![("c1", 1, 10), ("c2", 1, 11), ("c1", 1, 10), ("c1", 1, 10)],
            )
            .unwrap();
        assert_eq!(outcome.applied, 2);
        assert_eq!(outcome.duplicates, 2);
        assert_eq!(outcome.new_acked_lsn, 11);
    }

    #[test]
    fn stale_lsn_is_rejected_but_change_already_applied_duplicates() {
        let log = ReconciliationLog::new();
        log.apply("siteA", vec![("c1", 1, 10), ("c2", 1, 11)]).unwrap();
        let outcome = log
            .apply("siteA", vec![("c0", 1, 5), ("c3", 1, 12)])
            .unwrap();
        assert_eq!(outcome.stale, 1);
        assert_eq!(outcome.applied, 1);
        assert_eq!(outcome.new_acked_lsn, 12);
    }

    #[test]
    fn checkpoint_tracks_lag() {
        let log = ReconciliationLog::new();
        log.apply("siteA", vec![("c1", 1, 10)]).unwrap();
        let cp = log.checkpoint("siteA").unwrap();
        assert_eq!(cp.local_lsn, 10);
        assert_eq!(cp.acked_lsn, 10);
        assert_eq!(cp.lag(), 0);
    }
}
