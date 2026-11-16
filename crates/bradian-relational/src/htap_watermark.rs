//! ADR 0128 — HTAP freshness watermark primitive.
//!
//! BRD-HTAP-002 ("columnar projection freshness contract") is
//! OPEN. This module ships the correctness PRIMITIVE a future
//! HTAP coordinator can call to track the lag between the
//! transactional engine's commit LSN and the analytical
//! engine's projected LSN, and to declare whether a given
//! analytical read satisfies a caller-requested freshness
//! bound.
//!
//! Honest scope:
//! - Tracks two LSN watermarks: `latest_committed` (OLTP
//!   side) and `latest_projected` (OLAP side).
//! - Both are monotone: `advance_*` with a lower value is a
//!   no-op (defensive against out-of-order callers).
//! - `is_fresh_as_of(lsn)` answers "has columnar caught up to
//!   LSN `lsn`?".
//! - `lag()` reports the current OLAP lag in LSN units.
//!
//! Out of scope:
//! - Actual columnar projection work — this is a bookkeeping
//!   primitive, not the projector.
//! - Per-table / per-partition watermarks — one global pair.
//!   Future ADR can add per-table watermarks when the
//!   catalog grows a stable table-partition identity.
//! - LSN-to-wall-clock conversion.
//! - Callback / wait-until-fresh blocking — poll-based only.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Thread-safe HTAP freshness watermark.
///
/// `Arc`-cloneable; all callers share one `AtomicU64` pair.
#[derive(Clone, Debug, Default)]
pub struct HtapWatermark {
    inner: Arc<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    latest_committed: AtomicU64,
    latest_projected: AtomicU64,
}

/// Snapshot of both LSNs captured without CAS; the two loads
/// may race but each is monotone, so readers see values that
/// could have been seen at *some* moment during the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatermarkSnapshot {
    pub latest_committed: u64,
    pub latest_projected: u64,
}

impl HtapWatermark {
    pub fn new() -> Self { Self::default() }

    /// ADR 0128 — advance the OLTP side. Monotone: a lower
    /// value is silently ignored. Returns `true` if the
    /// watermark moved forward.
    pub fn advance_committed(&self, lsn: u64) -> bool {
        advance_monotone(&self.inner.latest_committed, lsn)
    }

    /// ADR 0128 — advance the OLAP side. Monotone. Never
    /// permits `latest_projected > latest_committed` — a
    /// projected LSN above the committed watermark is
    /// nonsense and is clamped by returning `false`.
    pub fn advance_projected(&self, lsn: u64) -> bool {
        let committed = self.inner.latest_committed.load(Ordering::Acquire);
        if lsn > committed { return false; }
        advance_monotone(&self.inner.latest_projected, lsn)
    }

    pub fn latest_committed(&self) -> u64 {
        self.inner.latest_committed.load(Ordering::Acquire)
    }

    pub fn latest_projected(&self) -> u64 {
        self.inner.latest_projected.load(Ordering::Acquire)
    }

    pub fn snapshot(&self) -> WatermarkSnapshot {
        WatermarkSnapshot {
            latest_committed: self.latest_committed(),
            latest_projected: self.latest_projected(),
        }
    }

    /// Current analytical lag in LSN units. `latest_committed
    /// - latest_projected`, saturating at 0.
    pub fn lag(&self) -> u64 {
        let s = self.snapshot();
        s.latest_committed.saturating_sub(s.latest_projected)
    }

    /// Would a read "as of LSN `lsn`" see a fresh-enough
    /// columnar projection? True iff `latest_projected >=
    /// lsn`.
    pub fn is_fresh_as_of(&self, lsn: u64) -> bool {
        self.latest_projected() >= lsn
    }
}

fn advance_monotone(cell: &AtomicU64, lsn: u64) -> bool {
    let mut current = cell.load(Ordering::Acquire);
    loop {
        if lsn <= current { return false; }
        match cell.compare_exchange_weak(current, lsn, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(seen) => current = seen,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_zero() {
        let w = HtapWatermark::new();
        assert_eq!(w.latest_committed(), 0);
        assert_eq!(w.latest_projected(), 0);
        assert_eq!(w.lag(), 0);
    }

    #[test]
    fn committed_advances_monotonically() {
        let w = HtapWatermark::new();
        assert!(w.advance_committed(10));
        assert!(w.advance_committed(20));
        assert!(!w.advance_committed(15), "backward advance must be rejected");
        assert_eq!(w.latest_committed(), 20);
    }

    #[test]
    fn projected_cannot_exceed_committed() {
        let w = HtapWatermark::new();
        w.advance_committed(10);
        assert!(!w.advance_projected(20), "projected > committed must be rejected");
        assert_eq!(w.latest_projected(), 0);
    }

    #[test]
    fn projected_can_catch_up_to_committed() {
        let w = HtapWatermark::new();
        w.advance_committed(100);
        assert!(w.advance_projected(100));
        assert_eq!(w.lag(), 0);
    }

    #[test]
    fn lag_reports_difference() {
        let w = HtapWatermark::new();
        w.advance_committed(100);
        w.advance_projected(60);
        assert_eq!(w.lag(), 40);
    }

    #[test]
    fn is_fresh_as_of_matches_projected_watermark() {
        let w = HtapWatermark::new();
        w.advance_committed(100);
        w.advance_projected(50);
        assert!(w.is_fresh_as_of(50));
        assert!(w.is_fresh_as_of(49));
        assert!(!w.is_fresh_as_of(51));
    }

    #[test]
    fn snapshot_captures_both_sides() {
        let w = HtapWatermark::new();
        w.advance_committed(100);
        w.advance_projected(60);
        let s = w.snapshot();
        assert_eq!(s.latest_committed, 100);
        assert_eq!(s.latest_projected, 60);
    }

    #[test]
    fn clone_shares_the_same_inner() {
        let a = HtapWatermark::new();
        let b = a.clone();
        a.advance_committed(42);
        assert_eq!(b.latest_committed(), 42);
    }

    #[test]
    fn concurrent_advances_are_safe() {
        use std::thread;
        let w = HtapWatermark::new();
        w.advance_committed(10_000);
        let handles: Vec<_> = (0..8).map(|t| {
            let w = w.clone();
            thread::spawn(move || {
                for i in 0..1000u64 {
                    let _ = w.advance_projected(t * 1000 + i);
                }
            })
        }).collect();
        for h in handles { h.join().unwrap(); }
        // Final value is monotone; must be <= committed.
        let p = w.latest_projected();
        assert!(p <= 10_000);
    }

    #[test]
    fn lag_saturates_at_zero() {
        let w = HtapWatermark::new();
        w.advance_committed(100);
        w.advance_projected(100);
        assert_eq!(w.lag(), 0);
    }
}
