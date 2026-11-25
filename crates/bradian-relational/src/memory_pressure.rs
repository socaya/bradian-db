//! ADR 0327 — Memory-pressure fault harness.
//!
//! Dynamic admission bookkeeping + injectable failure
//! policies so a test harness can simulate low-memory
//! conditions regardless of the static `ScanLimits`
//! cap. Each admission site that opts in calls
//! `policy.admit(bytes)` before cloning / allocating,
//! and the matching `release` on drop (or `Guard` auto-
//! releases via RAII).

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum MemoryPressureError {
    #[error("memory budget exceeded: would admit {admit} bytes, in_flight={in_flight}, limit={limit}")]
    BudgetExceeded { admit: usize, in_flight: usize, limit: usize },
    #[error("fault policy fired at cumulative {admitted} + {admit} > trip={trip}")]
    InjectedFault { admit: usize, admitted: usize, trip: usize },
}

pub trait MemoryPressurePolicy: Send + Sync {
    fn admit(&self, bytes: usize)   -> Result<(), MemoryPressureError>;
    fn release(&self, bytes: usize);
    fn in_flight(&self) -> usize;
}

/// Never refuses. Default.
#[derive(Debug, Default)]
pub struct UnboundedMemory {
    in_flight: AtomicUsize,
}
impl MemoryPressurePolicy for UnboundedMemory {
    fn admit(&self, bytes: usize) -> Result<(), MemoryPressureError> {
        self.in_flight.fetch_add(bytes, Ordering::SeqCst);
        Ok(())
    }
    fn release(&self, bytes: usize) {
        loop {
            let cur = self.in_flight.load(Ordering::SeqCst);
            let next = cur.saturating_sub(bytes);
            if self.in_flight.compare_exchange(cur, next,
                Ordering::SeqCst, Ordering::SeqCst).is_ok() { return; }
        }
    }
    fn in_flight(&self) -> usize { self.in_flight.load(Ordering::SeqCst) }
}

/// Fixed byte budget. Admission fails when `in_flight +
/// bytes > limit`. Concurrent admitters share the budget
/// via CAS so two threads racing to overflow cannot both
/// succeed.
#[derive(Debug)]
pub struct FixedBudget {
    limit:     usize,
    in_flight: AtomicUsize,
}

impl FixedBudget {
    pub fn new(limit: usize) -> Self {
        Self { limit, in_flight: AtomicUsize::new(0) }
    }
    pub fn limit(&self) -> usize { self.limit }
}

impl MemoryPressurePolicy for FixedBudget {
    fn admit(&self, bytes: usize) -> Result<(), MemoryPressureError> {
        if bytes == 0 { return Ok(()); }
        loop {
            let cur = self.in_flight.load(Ordering::SeqCst);
            let next = cur.saturating_add(bytes);
            if next > self.limit {
                return Err(MemoryPressureError::BudgetExceeded {
                    admit: bytes, in_flight: cur, limit: self.limit });
            }
            if self.in_flight.compare_exchange(cur, next,
                Ordering::SeqCst, Ordering::SeqCst).is_ok() { return Ok(()); }
        }
    }
    fn release(&self, bytes: usize) {
        loop {
            let cur = self.in_flight.load(Ordering::SeqCst);
            let next = cur.saturating_sub(bytes);
            if self.in_flight.compare_exchange(cur, next,
                Ordering::SeqCst, Ordering::SeqCst).is_ok() { return; }
        }
    }
    fn in_flight(&self) -> usize { self.in_flight.load(Ordering::SeqCst) }
}

/// Injectable one-shot fault: the Nth `admit` that would
/// push cumulative-admitted past `trip` is refused once;
/// subsequent admits succeed again. Cumulative is
/// separate from `in_flight` so a successful release does
/// not reset the trip counter.
#[derive(Debug)]
pub struct FailAfterBytes {
    trip:        usize,
    cumulative:  AtomicUsize,
    in_flight:   AtomicUsize,
    fired:       std::sync::atomic::AtomicBool,
}

impl FailAfterBytes {
    pub fn new(trip: usize) -> Self {
        Self {
            trip,
            cumulative: AtomicUsize::new(0),
            in_flight:  AtomicUsize::new(0),
            fired:      std::sync::atomic::AtomicBool::new(false),
        }
    }
    pub fn has_fired(&self) -> bool {
        self.fired.load(Ordering::SeqCst)
    }
}

impl MemoryPressurePolicy for FailAfterBytes {
    fn admit(&self, bytes: usize) -> Result<(), MemoryPressureError> {
        if bytes == 0 { return Ok(()); }
        // Tentatively increment cumulative; if we cross the
        // trip for the first time, fire the fault once.
        let prior = self.cumulative.fetch_add(bytes, Ordering::SeqCst);
        if prior.saturating_add(bytes) > self.trip
           && !self.fired.swap(true, Ordering::SeqCst) {
            return Err(MemoryPressureError::InjectedFault {
                admit: bytes, admitted: prior, trip: self.trip });
        }
        self.in_flight.fetch_add(bytes, Ordering::SeqCst);
        Ok(())
    }
    fn release(&self, bytes: usize) {
        loop {
            let cur = self.in_flight.load(Ordering::SeqCst);
            let next = cur.saturating_sub(bytes);
            if self.in_flight.compare_exchange(cur, next,
                Ordering::SeqCst, Ordering::SeqCst).is_ok() { return; }
        }
    }
    fn in_flight(&self) -> usize { self.in_flight.load(Ordering::SeqCst) }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryPressureStats {
    pub admitted_cumulative: u64,
    pub peak_in_flight:      u64,
    pub failures:            u64,
}

/// Observability wrapper. Counts cumulative admitted
/// bytes, peak observed in-flight, and failure count.
pub struct MemoryPressureHarness {
    policy:              Box<dyn MemoryPressurePolicy>,
    admitted_cumulative: AtomicU64,
    peak_in_flight:      AtomicU64,
    failures:            AtomicU64,
}

impl std::fmt::Debug for MemoryPressureHarness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryPressureHarness")
         .field("stats", &self.stats()).finish()
    }
}

impl MemoryPressureHarness {
    pub fn new(policy: impl MemoryPressurePolicy + 'static) -> Self {
        Self {
            policy: Box::new(policy),
            admitted_cumulative: AtomicU64::new(0),
            peak_in_flight:      AtomicU64::new(0),
            failures:            AtomicU64::new(0),
        }
    }

    pub fn admit(&self, bytes: usize) -> Result<MemoryPressureGuard<'_>, MemoryPressureError> {
        match self.policy.admit(bytes) {
            Ok(()) => {
                self.admitted_cumulative.fetch_add(bytes as u64, Ordering::SeqCst);
                let now = self.policy.in_flight() as u64;
                let mut peak = self.peak_in_flight.load(Ordering::SeqCst);
                while now > peak {
                    match self.peak_in_flight.compare_exchange(peak, now,
                        Ordering::SeqCst, Ordering::SeqCst) {
                        Ok(_)  => break,
                        Err(e) => peak = e,
                    }
                }
                Ok(MemoryPressureGuard { harness: self, bytes })
            }
            Err(e) => {
                self.failures.fetch_add(1, Ordering::SeqCst);
                Err(e)
            }
        }
    }

    pub fn stats(&self) -> MemoryPressureStats {
        MemoryPressureStats {
            admitted_cumulative: self.admitted_cumulative.load(Ordering::SeqCst),
            peak_in_flight:      self.peak_in_flight.load(Ordering::SeqCst),
            failures:            self.failures.load(Ordering::SeqCst),
        }
    }

    pub fn in_flight(&self) -> usize { self.policy.in_flight() }
}

/// RAII guard: releases the admitted bytes on drop.
pub struct MemoryPressureGuard<'a> {
    harness: &'a MemoryPressureHarness,
    bytes:   usize,
}

impl std::fmt::Debug for MemoryPressureGuard<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryPressureGuard").field("bytes", &self.bytes).finish()
    }
}

impl Drop for MemoryPressureGuard<'_> {
    fn drop(&mut self) {
        self.harness.policy.release(self.bytes);
    }
}

impl MemoryPressureGuard<'_> {
    pub fn bytes(&self) -> usize { self.bytes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn unbounded_never_fails_but_tracks_in_flight() {
        let p = UnboundedMemory::default();
        p.admit(100).unwrap();
        p.admit(200).unwrap();
        assert_eq!(p.in_flight(), 300);
        p.release(100);
        assert_eq!(p.in_flight(), 200);
    }

    #[test]
    fn fixed_budget_refuses_past_cap_and_recovers_after_release() {
        let p = FixedBudget::new(1000);
        p.admit(400).unwrap();
        p.admit(500).unwrap();
        assert_eq!(p.in_flight(), 900);
        // Over cap:
        let err = p.admit(200).unwrap_err();
        assert!(matches!(err, MemoryPressureError::BudgetExceeded { .. }));
        assert_eq!(p.in_flight(), 900,
            "failed admit must not bump in_flight");
        p.release(400);
        // Now 200 fits.
        p.admit(200).unwrap();
        assert_eq!(p.in_flight(), 700);
    }

    #[test]
    fn fail_after_bytes_trips_once_then_recovers() {
        let p = FailAfterBytes::new(500);
        // First two admits cumulate to 300, no trip.
        p.admit(200).unwrap();
        p.admit(100).unwrap();
        assert!(!p.has_fired());
        // Third admit crosses 500 → one-shot fault.
        let err = p.admit(300).unwrap_err();
        assert!(matches!(err, MemoryPressureError::InjectedFault { .. }));
        assert!(p.has_fired());
        // Subsequent admits succeed.
        p.admit(100).unwrap();
    }

    #[test]
    fn guard_releases_in_flight_on_drop() {
        let h = MemoryPressureHarness::new(FixedBudget::new(500));
        {
            let _g = h.admit(300).unwrap();
            assert_eq!(h.in_flight(), 300);
        }
        assert_eq!(h.in_flight(), 0,
            "guard drop must release the admitted bytes");
    }

    #[test]
    fn harness_stats_update_on_admit_and_fail() {
        let h = MemoryPressureHarness::new(FixedBudget::new(500));
        let _g1 = h.admit(100).unwrap();
        let _g2 = h.admit(200).unwrap();
        // Failure
        let err = h.admit(500).unwrap_err();
        assert!(matches!(err, MemoryPressureError::BudgetExceeded { .. }));
        let s = h.stats();
        assert_eq!(s.admitted_cumulative, 300,
            "cumulative counts only successful admits");
        assert_eq!(s.peak_in_flight, 300);
        assert_eq!(s.failures, 1);
    }

    #[test]
    fn concurrent_admitters_share_the_budget_correctly() {
        let p = Arc::new(FixedBudget::new(1000));
        let mut handles = Vec::new();
        for _ in 0..10 {
            let p = p.clone();
            handles.push(thread::spawn(move || {
                // Each thread tries to admit 200 bytes
                // twice. Only 5 of the 20 attempts can
                // possibly succeed (5 * 200 = 1000).
                let mut ok = 0;
                for _ in 0..2 {
                    if p.admit(200).is_ok() { ok += 1; }
                }
                ok
            }));
        }
        let total_ok: u32 = handles.into_iter()
            .map(|h| h.join().unwrap()).sum();
        assert_eq!(total_ok, 5,
            "concurrent admitters should sum to exactly budget/size");
        assert_eq!(p.in_flight(), 1000);
    }

    #[test]
    fn zero_byte_admit_is_a_no_op() {
        let p = FixedBudget::new(0);
        // Zero-byte admit succeeds even at a zero budget.
        p.admit(0).unwrap();
        assert_eq!(p.in_flight(), 0);
    }

    #[test]
    fn release_of_more_than_admitted_saturates_without_underflow() {
        let p = FixedBudget::new(500);
        p.admit(100).unwrap();
        p.release(9999);
        assert_eq!(p.in_flight(), 0,
            "release must saturate, never underflow");
    }
}
