//! ADR 0129 — local driver facade trait.
//!
//! BRD-DRV-001 ("portable client driver API") is OPEN.
//! This module ships the minimal trait a future client
//! library can depend on without caring which SQL engine
//! backs it: `SqlEngine`, `MvccSqlEngine`, `DomainSqlEngine`,
//! `HistorySqlEngine`, or a future network adapter.
//!
//! Honest scope:
//! - `Driver` trait with a single `execute` method.
//! - `DriverResponse` enum covering the three canonical
//!   outcomes: affected-row count, a row set, or a
//!   statement that produced no rows.
//! - `NoopDriver` reference implementation that records
//!   the submitted statements; useful as a stub in tests
//!   that need a Driver but not real execution.
//!
//! Out of scope (follow-ups):
//! - **Engine adapters** — wrapping the existing SQL
//!   engines behind this trait requires per-engine
//!   glue code (schema pin, session mode, snapshot
//!   LSN, savepoint lifecycle). Ship each adapter in
//!   its own ADR.
//! - **Async / network driver** — tokio + wire protocol
//!   belongs in a dedicated `bradian-wire` crate.
//! - **Prepared statements** — one-shot execute only.
//! - **Transaction scope** — a `Driver::begin` / `commit`
//!   / `rollback` triplet is deferred until the session
//!   lifecycle design is settled across engines.
//! - **Typed row projection** — `DriverRow` is raw
//!   `Vec<Value>`; future ADR can add a column-name
//!   schema envelope.

use thiserror::Error;

use crate::typed_table::Value;

#[derive(Debug, Error)]
pub enum DriverError {
    #[error("driver not yet backed by an engine (stub)")]
    NotWired,
    #[error("sql error: {0}")]
    Sql(String),
    #[error("resource exhausted: {0}")]
    Resource(String),
}

pub type DriverResult<T> = std::result::Result<T, DriverError>;

/// Row payload returned by read statements. Column names
/// are paired with values by position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverRow {
    pub columns: Vec<String>,
    pub values: Vec<Value>,
}

/// Canonical outcome of `Driver::execute`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriverResponse {
    /// Statement produced no result set (e.g., DDL, a
    /// side-effectless operation).
    Empty,
    /// Statement modified rows; carries the count.
    Affected { rows: u64 },
    /// Statement produced a row set.
    Rows(Vec<DriverRow>),
}

/// Portable client driver facade. Implementations wrap one
/// of the SQL engines (or forward over the wire).
pub trait Driver: Send + Sync + std::fmt::Debug {
    /// Execute a single SQL statement. Implementations
    /// must NOT accept a batch — multi-statement execution
    /// belongs in a future session API.
    fn execute(&self, sql: &str) -> DriverResult<DriverResponse>;
}

/// ADR 0129 — stub driver that never executes anything.
/// Records every submitted statement for later inspection
/// (useful as a test double). Returns `NotWired`.
#[derive(Debug, Default)]
pub struct NoopDriver {
    log: parking_lot::Mutex<Vec<String>>,
}

impl NoopDriver {
    pub fn new() -> Self { Self::default() }

    /// Snapshot of every statement submitted so far.
    pub fn log(&self) -> Vec<String> { self.log.lock().clone() }
}

impl Driver for NoopDriver {
    fn execute(&self, sql: &str) -> DriverResult<DriverResponse> {
        self.log.lock().push(sql.to_owned());
        Err(DriverError::NotWired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_driver_records_submissions_and_returns_not_wired() {
        let d = NoopDriver::new();
        assert!(matches!(d.execute("SELECT 1"), Err(DriverError::NotWired)));
        assert!(matches!(d.execute("CREATE TABLE t (a INT)"),
                         Err(DriverError::NotWired)));
        let log = d.log();
        assert_eq!(log, vec!["SELECT 1".to_string(),
                             "CREATE TABLE t (a INT)".to_string()]);
    }

    #[test]
    fn noop_driver_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NoopDriver>();
    }

    #[test]
    fn driver_trait_is_object_safe() {
        let d: Box<dyn Driver> = Box::new(NoopDriver::new());
        let _ = d.execute("SELECT 1");
    }

    #[test]
    fn driver_response_variants_are_distinct() {
        let empty = DriverResponse::Empty;
        let affected = DriverResponse::Affected { rows: 3 };
        let rows = DriverResponse::Rows(vec![DriverRow {
            columns: vec!["id".into()],
            values: vec![Value::Int64(1)],
        }]);
        assert_ne!(empty, affected);
        assert_ne!(affected, rows);
        assert_ne!(empty, rows);
    }

    #[test]
    fn noop_driver_log_is_cloned_snapshot() {
        let d = NoopDriver::new();
        let _ = d.execute("A");
        let snap1 = d.log();
        let _ = d.execute("B");
        let snap2 = d.log();
        // Snapshot is immutable — not a live view.
        assert_eq!(snap1, vec!["A".to_string()]);
        assert_eq!(snap2, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn driver_row_round_trips_values() {
        let r = DriverRow {
            columns: vec!["a".into(), "b".into()],
            values: vec![Value::Int32(1), Value::String("x".into())],
        };
        assert_eq!(r.columns.len(), r.values.len());
    }
}
