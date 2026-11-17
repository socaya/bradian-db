//! ADR 0141 — query budget primitive.
//!
//! BRD-RUNTIME-003 "query-level timeout + resource bound"
//! is OPEN. ADR 0114 ships a lock-acquisition timeout
//! policy; ADR 0041/0044 bound scan key/byte counts.
//! The gap was a per-QUERY budget object the engine can
//! consult at admission checkpoints: wall-clock deadline,
//! rows scanned, bytes materialized, inner-loop iterations.
//!
//! Honest scope:
//! - `QueryBudget` holds four knobs: max_duration,
//!   max_rows_scanned, max_bytes_materialized,
//!   max_iterations.
//! - `BudgetTracker` captures a monotonic `Instant` at
//!   construction and offers `charge_*` methods that
//!   return `Err(BudgetExceeded)` when any knob breaches.
//! - Pure counter arithmetic + a single `Instant::now()`
//!   call per check. No background threads.
//!
//! Out of scope (follow-ups):
//! - Cancellation / interrupt — breaching the budget
//!   returns an error at the next charge point; a running
//!   tight loop without a charge point won't be
//!   interrupted mid-iteration.
//! - Per-engine integration — the engines don't
//!   consult this yet.
//! - Hierarchical budgets (parent / child plan nodes).
//! - CPU-time budget (needs libc getrusage).
//! - Priority / fair scheduling.

use std::time::{Duration, Instant};

use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryBudget {
    pub max_duration: Option<Duration>,
    pub max_rows_scanned: Option<u64>,
    pub max_bytes_materialized: Option<u64>,
    pub max_iterations: Option<u64>,
}

impl Default for QueryBudget {
    /// Conservative default: 30 s, 10 M rows, 1 GiB
    /// materialized, 1 G iterations. Callers tighten per
    /// workload.
    fn default() -> Self {
        Self {
            max_duration: Some(Duration::from_secs(30)),
            max_rows_scanned: Some(10_000_000),
            max_bytes_materialized: Some(1 << 30),
            max_iterations: Some(1_000_000_000),
        }
    }
}

impl QueryBudget {
    /// Unbounded on every knob — useful for admin tools
    /// that are their own supervisor.
    pub fn unbounded() -> Self {
        Self {
            max_duration: None,
            max_rows_scanned: None,
            max_bytes_materialized: None,
            max_iterations: None,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BudgetExceeded {
    #[error("query exceeded duration budget: elapsed {elapsed_ms} ms > max {max_ms} ms")]
    Duration { elapsed_ms: u64, max_ms: u64 },
    #[error("query exceeded row scan budget: scanned {scanned} > max {max}")]
    Rows { scanned: u64, max: u64 },
    #[error("query exceeded materialization budget: bytes {bytes} > max {max}")]
    Bytes { bytes: u64, max: u64 },
    #[error("query exceeded iteration budget: iterations {iterations} > max {max}")]
    Iterations { iterations: u64, max: u64 },
}

pub type BudgetResult<T> = std::result::Result<T, BudgetExceeded>;

/// ADR 0141 — stateful tracker a caller charges as the
/// query progresses.
#[derive(Debug)]
pub struct BudgetTracker {
    budget: QueryBudget,
    start: Instant,
    rows_scanned: u64,
    bytes_materialized: u64,
    iterations: u64,
}

/// ADR 0148 — approximate materialized byte size of one
/// row. Used by the SQL engines' SELECT paths to charge
/// the budget's `max_bytes_materialized` knob. Size
/// matches the stored width of each `Value` variant, not
/// the Rust `std::mem::size_of` footprint — the caller
/// cares about "bytes the query produced", not
/// alignment / discriminant padding.
pub fn row_bytes(row: &[crate::typed_table::Value]) -> u64 {
    use crate::typed_table::Value;
    row.iter().map(|v| match v {
        Value::Null => 1,
        Value::Boolean(_) => 1,
        Value::Int32(_) => 4,
        Value::Int64(_) => 8,
        Value::Decimal(_) => 16,
        Value::TimestampTz(_) => 8,
        Value::Uuid(_) => 16,
        Value::String(s) => s.len() as u64,
    }).sum()
}

/// ADR 0148 — sum of `row_bytes` across a row slice.
pub fn rows_bytes(rows: &[Vec<crate::typed_table::Value>]) -> u64 {
    rows.iter().map(|r| row_bytes(r)).sum()
}

impl BudgetTracker {
    pub fn new(budget: QueryBudget) -> Self {
        Self {
            budget, start: Instant::now(),
            rows_scanned: 0, bytes_materialized: 0, iterations: 0,
        }
    }

    pub fn budget(&self) -> &QueryBudget { &self.budget }
    pub fn rows_scanned(&self) -> u64 { self.rows_scanned }
    pub fn bytes_materialized(&self) -> u64 { self.bytes_materialized }
    pub fn iterations(&self) -> u64 { self.iterations }
    pub fn elapsed(&self) -> Duration { self.start.elapsed() }

    /// Add `n` to rows scanned; error if over budget.
    pub fn charge_rows(&mut self, n: u64) -> BudgetResult<()> {
        self.rows_scanned = self.rows_scanned.saturating_add(n);
        if let Some(m) = self.budget.max_rows_scanned {
            if self.rows_scanned > m {
                return Err(BudgetExceeded::Rows { scanned: self.rows_scanned, max: m });
            }
        }
        Ok(())
    }

    pub fn charge_bytes(&mut self, n: u64) -> BudgetResult<()> {
        self.bytes_materialized = self.bytes_materialized.saturating_add(n);
        if let Some(m) = self.budget.max_bytes_materialized {
            if self.bytes_materialized > m {
                return Err(BudgetExceeded::Bytes { bytes: self.bytes_materialized, max: m });
            }
        }
        Ok(())
    }

    pub fn charge_iteration(&mut self) -> BudgetResult<()> {
        self.iterations = self.iterations.saturating_add(1);
        if let Some(m) = self.budget.max_iterations {
            if self.iterations > m {
                return Err(BudgetExceeded::Iterations { iterations: self.iterations, max: m });
            }
        }
        Ok(())
    }

    /// Check only the wall-clock budget — cheap enough to
    /// call from an inner loop every few thousand
    /// iterations.
    pub fn check_deadline(&self) -> BudgetResult<()> {
        if let Some(m) = self.budget.max_duration {
            let e = self.elapsed();
            if e > m {
                return Err(BudgetExceeded::Duration {
                    elapsed_ms: e.as_millis().min(u64::MAX as u128) as u64,
                    max_ms: m.as_millis().min(u64::MAX as u128) as u64,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn default_budget_has_all_bounds() {
        let b = QueryBudget::default();
        assert!(b.max_duration.is_some());
        assert!(b.max_rows_scanned.is_some());
        assert!(b.max_bytes_materialized.is_some());
        assert!(b.max_iterations.is_some());
    }

    #[test]
    fn unbounded_budget_has_no_bounds() {
        let b = QueryBudget::unbounded();
        assert!(b.max_duration.is_none());
        assert!(b.max_rows_scanned.is_none());
        assert!(b.max_bytes_materialized.is_none());
        assert!(b.max_iterations.is_none());
    }

    #[test]
    fn charge_rows_within_budget_ok() {
        let mut t = BudgetTracker::new(QueryBudget {
            max_rows_scanned: Some(100),
            ..QueryBudget::unbounded()
        });
        t.charge_rows(50).unwrap();
        t.charge_rows(50).unwrap();
        assert_eq!(t.rows_scanned(), 100);
    }

    #[test]
    fn charge_rows_over_budget_errors() {
        let mut t = BudgetTracker::new(QueryBudget {
            max_rows_scanned: Some(100),
            ..QueryBudget::unbounded()
        });
        t.charge_rows(50).unwrap();
        assert!(matches!(t.charge_rows(51),
            Err(BudgetExceeded::Rows { scanned: 101, max: 100 })));
    }

    #[test]
    fn charge_bytes_over_budget_errors() {
        let mut t = BudgetTracker::new(QueryBudget {
            max_bytes_materialized: Some(1024),
            ..QueryBudget::unbounded()
        });
        t.charge_bytes(512).unwrap();
        assert!(matches!(t.charge_bytes(513),
            Err(BudgetExceeded::Bytes { bytes: 1025, max: 1024 })));
    }

    #[test]
    fn charge_iteration_over_budget_errors() {
        let mut t = BudgetTracker::new(QueryBudget {
            max_iterations: Some(2),
            ..QueryBudget::unbounded()
        });
        t.charge_iteration().unwrap();
        t.charge_iteration().unwrap();
        assert!(matches!(t.charge_iteration(),
            Err(BudgetExceeded::Iterations { iterations: 3, max: 2 })));
    }

    #[test]
    fn check_deadline_fires_when_exceeded() {
        let t = BudgetTracker::new(QueryBudget {
            max_duration: Some(Duration::from_millis(10)),
            ..QueryBudget::unbounded()
        });
        sleep(Duration::from_millis(20));
        assert!(matches!(t.check_deadline(),
            Err(BudgetExceeded::Duration { .. })));
    }

    #[test]
    fn check_deadline_within_budget_ok() {
        let t = BudgetTracker::new(QueryBudget {
            max_duration: Some(Duration::from_secs(5)),
            ..QueryBudget::unbounded()
        });
        t.check_deadline().unwrap();
    }

    #[test]
    fn unbounded_tracker_never_errors() {
        let mut t = BudgetTracker::new(QueryBudget::unbounded());
        t.charge_rows(u64::MAX / 2).unwrap();
        t.charge_bytes(u64::MAX / 2).unwrap();
        t.charge_iteration().unwrap();
        t.check_deadline().unwrap();
    }

    #[test]
    fn accessors_report_cumulative_state() {
        let mut t = BudgetTracker::new(QueryBudget::unbounded());
        t.charge_rows(7).unwrap();
        t.charge_bytes(42).unwrap();
        t.charge_iteration().unwrap();
        t.charge_iteration().unwrap();
        assert_eq!(t.rows_scanned(), 7);
        assert_eq!(t.bytes_materialized(), 42);
        assert_eq!(t.iterations(), 2);
    }

    #[test]
    fn saturating_add_prevents_overflow() {
        let mut t = BudgetTracker::new(QueryBudget::unbounded());
        t.charge_rows(u64::MAX).unwrap();
        t.charge_rows(100).unwrap();  // saturates, does not panic
        assert_eq!(t.rows_scanned(), u64::MAX);
    }

    #[test]
    fn row_bytes_counts_variants_correctly() {
        use crate::typed_table::Value;
        let r = [
            Value::Null,                 // 1
            Value::Boolean(true),         // 1
            Value::Int32(0),              // 4
            Value::Int64(0),              // 8
            Value::Decimal(0),            // 16
            Value::TimestampTz(0),        // 8
            Value::Uuid([0; 16]),         // 16
            Value::String("abcde".into()),// 5
        ];
        assert_eq!(row_bytes(&r), 1 + 1 + 4 + 8 + 16 + 8 + 16 + 5);
    }

    #[test]
    fn rows_bytes_sums_across_rows() {
        use crate::typed_table::Value;
        let rows = vec![
            vec![Value::Int32(0), Value::Int32(0)],
            vec![Value::String("x".into()), Value::String("yz".into())],
        ];
        assert_eq!(rows_bytes(&rows), 4 + 4 + 1 + 2);
    }

    #[test]
    fn empty_row_is_zero_bytes() {
        let r: Vec<crate::typed_table::Value> = vec![];
        assert_eq!(row_bytes(&r), 0);
    }
}
