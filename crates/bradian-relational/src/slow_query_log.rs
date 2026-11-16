//! ADR 0117 — in-process slow-query log.
//!
//! When a SQL statement's elapsed time exceeds the operator-
//! configured threshold, the statement + principal + elapsed
//! are appended to an in-memory ring buffer. Readers can
//! inspect the buffer for diagnostic purposes.
//!
//! Non-goals:
//! - Durable storage (use ADR 0104 audit log for that).
//! - Automatic collection — callers invoke `record()`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlowQueryRecord {
    pub timestamp_ms: i64,
    pub principal_id: String,
    pub sql: String,
    pub elapsed_us: u64,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Ok,
    Err(String),
}

#[derive(Debug, Clone)]
pub struct SlowQueryLog {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug)]
struct Inner {
    threshold_us: u64,
    capacity: usize,
    buffer: VecDeque<SlowQueryRecord>,
}

impl SlowQueryLog {
    /// Create a log with the given threshold (slow if elapsed
    /// > threshold) and ring-buffer capacity.
    pub fn new(threshold: Duration, capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                threshold_us: threshold.as_micros() as u64,
                capacity,
                buffer: VecDeque::with_capacity(capacity),
            })),
        }
    }

    /// Record a statement if it's slow. Oldest record evicted
    /// when capacity reached.
    pub fn record(&self, record: SlowQueryRecord) {
        let mut inner = self.inner.lock();
        if record.elapsed_us <= inner.threshold_us { return; }
        let cap = inner.capacity;
        if inner.buffer.len() >= cap {
            inner.buffer.pop_front();
        }
        inner.buffer.push_back(record);
    }

    /// Copy of every retained record.
    pub fn snapshot(&self) -> Vec<SlowQueryRecord> {
        self.inner.lock().buffer.iter().cloned().collect()
    }

    pub fn threshold_us(&self) -> u64 { self.inner.lock().threshold_us }
    pub fn capacity(&self) -> usize { self.inner.lock().capacity }

    /// Clear the buffer.
    pub fn clear(&self) {
        self.inner.lock().buffer.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(elapsed_us: u64) -> SlowQueryRecord {
        SlowQueryRecord {
            timestamp_ms: 0,
            principal_id: "root".into(),
            sql: "SELECT 1".into(),
            elapsed_us,
            outcome: Outcome::Ok,
        }
    }

    #[test]
    fn under_threshold_does_not_record() {
        let log = SlowQueryLog::new(Duration::from_millis(100), 10);
        log.record(rec(50_000));   // 50ms
        assert!(log.snapshot().is_empty());
    }

    #[test]
    fn over_threshold_records() {
        let log = SlowQueryLog::new(Duration::from_millis(100), 10);
        log.record(rec(500_000));  // 500ms
        assert_eq!(log.snapshot().len(), 1);
    }

    #[test]
    fn ring_buffer_evicts_oldest_at_capacity() {
        let log = SlowQueryLog::new(Duration::from_millis(1), 3);
        for i in 0..5 {
            log.record(SlowQueryRecord {
                timestamp_ms: i,
                principal_id: "p".into(),
                sql: format!("q{i}"),
                elapsed_us: 10_000, // always over 1ms threshold
                outcome: Outcome::Ok,
            });
        }
        let snap = log.snapshot();
        assert_eq!(snap.len(), 3);
        // Oldest two evicted.
        assert_eq!(snap[0].sql, "q2");
        assert_eq!(snap[2].sql, "q4");
    }

    #[test]
    fn clear_empties_the_buffer() {
        let log = SlowQueryLog::new(Duration::from_millis(1), 10);
        log.record(rec(1_000_000));
        assert_eq!(log.snapshot().len(), 1);
        log.clear();
        assert!(log.snapshot().is_empty());
    }

    #[test]
    fn multiple_handles_share_state() {
        let log = SlowQueryLog::new(Duration::from_millis(1), 10);
        let clone = log.clone();
        clone.record(rec(500_000));
        assert_eq!(log.snapshot().len(), 1);
    }
}
