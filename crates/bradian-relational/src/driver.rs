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

    /// ADR 0161 — execute a parameterised prepared
    /// statement. The default implementation validates
    /// the envelope, binds parameters into literal
    /// positions via `bind_literals`, and delegates to
    /// `execute(sql)`. Native-backed drivers can override
    /// for caching / server-side plans.
    fn execute_prepared(
        &self,
        stmt: &crate::prepared::PreparedStatement,
    ) -> DriverResult<DriverResponse> {
        stmt.validate().map_err(|e| DriverError::Sql(e.to_string()))?;
        let sql = bind_literals(&stmt.sql, &stmt.params)
            .map_err(DriverError::Sql)?;
        self.execute(&sql)
    }
}

/// ADR 0161 — textual parameter binding. Replaces `$N`
/// positional placeholders with literal SQL. Not a
/// substitute for server-side prepared plans, but
/// sufficient for the current driver surface where the
/// backing engine re-parses every statement.
///
/// Honest scope: minimal literal emission. No SQL
/// injection protection beyond parameter typing — the
/// caller is responsible for validating parameter
/// provenance before passing them through a trusted
/// `Driver`.
pub fn bind_literals(
    template: &str,
    params: &[crate::typed_table::Value],
) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            let mut j = i + 1;
            let mut n: u32 = 0;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                n = n.saturating_mul(10)
                    .saturating_add((bytes[j] - b'0') as u32);
                j += 1;
            }
            if n == 0 { return Err("placeholder $0 is invalid".into()); }
            if (n as usize) > params.len() {
                return Err(format!("placeholder ${n} out of range"));
            }
            out.push_str(&value_to_sql(&params[(n - 1) as usize]));
            i = j;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    Ok(out)
}

fn value_to_sql(v: &crate::typed_table::Value) -> String {
    use crate::typed_table::Value;
    match v {
        Value::Null => "NULL".into(),
        Value::Boolean(b) => if *b { "TRUE".into() } else { "FALSE".into() },
        Value::Int32(n) => n.to_string(),
        Value::Int64(n) => n.to_string(),
        Value::Decimal(d) => d.to_string(),
        Value::TimestampTz(micros) => micros.to_string(),
        Value::Uuid(bytes) => {
            let mut hex = String::with_capacity(32);
            for b in bytes { hex.push_str(&format!("{b:02x}")); }
            format!("'{hex}'")
        }
        Value::String(s) | Value::Json(s) => {
            // Single-quote-escape via doubling. ADR 0173 — JSON
            // renders as a quoted string literal; the SQL coerce
            // layer validates its content as serde_json-parsable
            // before it ever reaches this encoder.
            let escaped = s.replace('\'', "''");
            format!("'{escaped}'")
        }
    }
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

    #[test]
    fn bind_literals_substitutes_positional_placeholders() {
        let out = bind_literals(
            "SELECT * FROM t WHERE id = $1 AND name = $2",
            &[Value::Int64(42), Value::String("alice".into())],
        ).unwrap();
        assert_eq!(out, "SELECT * FROM t WHERE id = 42 AND name = 'alice'");
    }

    #[test]
    fn bind_literals_escapes_single_quotes_in_strings() {
        let out = bind_literals(
            "INSERT INTO t VALUES ($1)",
            &[Value::String("it's".into())],
        ).unwrap();
        assert_eq!(out, "INSERT INTO t VALUES ('it''s')");
    }

    #[test]
    fn bind_literals_rejects_out_of_range_placeholder() {
        let err = bind_literals(
            "SELECT $5", &[Value::Int64(1)],
        ).unwrap_err();
        assert!(err.contains("out of range"));
    }

    #[test]
    fn bind_literals_rejects_zero_indexed_placeholder() {
        let err = bind_literals("SELECT $0", &[]).unwrap_err();
        assert!(err.contains("$0"));
    }

    #[test]
    fn bind_literals_handles_null_boolean_and_uuid() {
        let out = bind_literals(
            "VALUES ($1, $2, $3, $4)",
            &[
                Value::Null,
                Value::Boolean(true),
                Value::Boolean(false),
                Value::Uuid([0xab; 16]),
            ],
        ).unwrap();
        assert!(out.contains("NULL"));
        assert!(out.contains("TRUE"));
        assert!(out.contains("FALSE"));
        assert!(out.contains("'abababababababababababababababab'"));
    }

    #[test]
    fn noop_driver_execute_prepared_routes_through_execute() {
        let d = NoopDriver::new();
        let stmt = crate::prepared::PreparedStatement::new(
            "SELECT $1",
            vec![Value::Int64(7)],
        );
        // NoopDriver returns NotWired, but the bind step
        // runs first and the submission log records the
        // bound SQL.
        let _ = d.execute_prepared(&stmt);
        let log = d.log();
        assert_eq!(log, vec!["SELECT 7".to_string()]);
    }
}
