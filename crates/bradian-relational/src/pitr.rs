//! ADR 0102 — PITR timestamp → LSN map.
//!
//! Operators often want "AS OF yesterday 14:00" rather than
//! "AS OF LSN 42 173". This module provides the lookup layer:
//! given a target wall-clock timestamp (ms since epoch) and a
//! recorded history of `(commit_time, lsn)` pairs, return the
//! latest LSN with `commit_time <= target`.
//!
//! Honest scope: the recording side is a thin append-only
//! log the engine or an external supervisor can fill. The
//! query side is monotone binary search. No WAL rewind.
//!
//! Non-goals:
//! - Durable storage of the time→LSN map itself. Callers who
//!   want persistence should serialise the `PointInTimeMap`
//!   bytes via serde (no new format defined here).
//! - Automatic recording from `commit_batch` — that requires
//!   per-engine integration.
//! - Monotonicity enforcement across clock-skew: the map
//!   rejects records whose timestamp decreases.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Wall-clock time of the commit, ms since Unix epoch.
    pub commit_time_ms: i64,
    /// Durable LSN that became visible at `commit_time_ms`.
    pub lsn: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PointInTimeMap {
    /// Sorted ascending by `commit_time_ms` and `lsn`. Both
    /// fields monotone-increase; record() rejects anything else.
    entries: Vec<Checkpoint>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PitrError {
    #[error("commit_time_ms must be non-decreasing: existing {existing} > new {new}")]
    TimestampRegression { existing: i64, new: i64 },
    #[error("lsn must be strictly increasing: existing {existing} >= new {new}")]
    LsnRegression { existing: u64, new: u64 },
    #[error("no checkpoint at or before {target_ms} ms; earliest is {earliest_ms}")]
    TargetBeforeEarliest { target_ms: i64, earliest_ms: i64 },
    #[error("point-in-time map is empty")]
    Empty,
}

pub type Result<T> = std::result::Result<T, PitrError>;

impl PointInTimeMap {
    pub fn new() -> Self { Self::default() }

    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn len(&self) -> usize { self.entries.len() }

    /// Append a new checkpoint. Both the timestamp and the LSN
    /// must strictly exceed the last recorded values (LSN) or
    /// non-decrease (timestamp — ties tolerated so a flurry of
    /// commits at the same millisecond works).
    pub fn record(&mut self, cp: Checkpoint) -> Result<()> {
        if let Some(last) = self.entries.last() {
            if cp.commit_time_ms < last.commit_time_ms {
                return Err(PitrError::TimestampRegression {
                    existing: last.commit_time_ms,
                    new: cp.commit_time_ms,
                });
            }
            if cp.lsn <= last.lsn {
                return Err(PitrError::LsnRegression {
                    existing: last.lsn,
                    new: cp.lsn,
                });
            }
        }
        self.entries.push(cp);
        Ok(())
    }

    /// Return the latest LSN whose commit time is at or before
    /// `target_ms`. Returns `TargetBeforeEarliest` when the
    /// target precedes every recorded checkpoint.
    pub fn lsn_as_of(&self, target_ms: i64) -> Result<u64> {
        if self.entries.is_empty() {
            return Err(PitrError::Empty);
        }
        // Binary search for the last entry with time <= target.
        let idx = self.entries.partition_point(|cp| cp.commit_time_ms <= target_ms);
        if idx == 0 {
            return Err(PitrError::TargetBeforeEarliest {
                target_ms,
                earliest_ms: self.entries[0].commit_time_ms,
            });
        }
        Ok(self.entries[idx - 1].lsn)
    }

    /// Return the full sequence (useful for persistence /
    /// operator diagnostics).
    pub fn entries(&self) -> &[Checkpoint] { &self.entries }

    /// Discard checkpoints older than `horizon_ms`. Preserves
    /// at least one entry so subsequent lookups don't trip
    /// `Empty`.
    pub fn truncate_before(&mut self, horizon_ms: i64) {
        if self.entries.len() <= 1 { return; }
        let keep_from = self.entries.partition_point(|cp| cp.commit_time_ms < horizon_ms)
            .saturating_sub(0)
            .min(self.entries.len() - 1);
        if keep_from > 0 {
            self.entries.drain(..keep_from);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp(t: i64, lsn: u64) -> Checkpoint { Checkpoint { commit_time_ms: t, lsn } }

    #[test]
    fn empty_map_lookup_errors() {
        let m = PointInTimeMap::new();
        assert!(matches!(m.lsn_as_of(1000), Err(PitrError::Empty)));
    }

    #[test]
    fn record_and_lookup_returns_latest_before_or_at_target() {
        let mut m = PointInTimeMap::new();
        m.record(cp(1000, 1)).unwrap();
        m.record(cp(2000, 2)).unwrap();
        m.record(cp(3000, 3)).unwrap();
        assert_eq!(m.lsn_as_of(1000).unwrap(), 1, "exact match at boundary");
        assert_eq!(m.lsn_as_of(1500).unwrap(), 1, "between boundaries → earlier");
        assert_eq!(m.lsn_as_of(2500).unwrap(), 2);
        assert_eq!(m.lsn_as_of(10_000).unwrap(), 3, "after last → last");
    }

    #[test]
    fn target_before_earliest_errors() {
        let mut m = PointInTimeMap::new();
        m.record(cp(1000, 1)).unwrap();
        let err = m.lsn_as_of(500).unwrap_err();
        assert!(matches!(err, PitrError::TargetBeforeEarliest { target_ms: 500, earliest_ms: 1000 }));
    }

    #[test]
    fn same_millisecond_allows_multiple_lsns() {
        let mut m = PointInTimeMap::new();
        m.record(cp(1000, 1)).unwrap();
        m.record(cp(1000, 2)).unwrap();  // same ms, higher LSN
        m.record(cp(1000, 3)).unwrap();
        assert_eq!(m.lsn_as_of(1000).unwrap(), 3, "last LSN at the millisecond");
    }

    #[test]
    fn regression_timestamps_and_lsns_rejected() {
        let mut m = PointInTimeMap::new();
        m.record(cp(1000, 1)).unwrap();
        assert!(matches!(m.record(cp(999, 2)), Err(PitrError::TimestampRegression { .. })));
        assert!(matches!(m.record(cp(2000, 1)), Err(PitrError::LsnRegression { .. })));
    }

    #[test]
    fn truncate_before_drops_old_entries_preserves_lookup_tail() {
        let mut m = PointInTimeMap::new();
        for i in 1..=5 { m.record(cp(i * 1000, i as u64)).unwrap(); }
        m.truncate_before(3500);
        assert!(m.len() <= 5);
        assert!(m.lsn_as_of(5000).is_ok(), "latest still queryable");
    }

    #[test]
    fn serde_roundtrip_preserves_order() {
        let mut m = PointInTimeMap::new();
        m.record(cp(1000, 1)).unwrap();
        m.record(cp(2000, 2)).unwrap();
        let bytes = serde_json::to_vec(&m).unwrap();
        let recovered: PointInTimeMap = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(recovered.entries(), m.entries());
    }
}
