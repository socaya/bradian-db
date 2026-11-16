//! ADR 0139 — prepared-statement envelope.
//!
//! BRD-DRV-002 "prepared statements" is OPEN. ADR 0129
//! ships the `Driver` trait with a single `execute(sql)`
//! method — no parameter binding. This module ships the
//! value-layer envelope a future prepared-statement path
//! will use: SQL template + positional parameter list,
//! serializable so it round-trips through an ADR 0137
//! wire frame.
//!
//! Honest scope:
//! - `PreparedStatement { sql, params }` carries the
//!   query text and a typed parameter list.
//! - Positional parameters only (`$1`, `$2`, …). Named
//!   parameters are a follow-up.
//! - Parameters use existing `Value` and so implicitly
//!   round-trip serde JSON for the wire.
//! - Static syntactic check: count the distinct `$N`
//!   placeholders in the SQL, reject if `N` exceeds the
//!   supplied parameters, flag unused parameters.
//!
//! Out of scope (follow-ups):
//! - Engine binding — execution path remains per-engine
//!   ADR work.
//! - Type pre-validation — binding against a column's
//!   `DataType` requires the catalog schema; the
//!   envelope only validates count + placeholder range.
//! - Named parameters.
//! - Caching / plan re-use.
//! - Lexer-aware placeholder scan — the current scan is
//!   a textual `$digits` grep, so `$1` appearing inside
//!   a string literal would be counted.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::typed_table::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedStatement {
    pub sql: String,
    pub params: Vec<Value>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PreparedError {
    #[error("sql references placeholder ${position} but only {supplied} parameter(s) were supplied")]
    PlaceholderOutOfRange { position: u32, supplied: usize },
    #[error("sql uses {used} placeholder(s) but {supplied} parameter(s) were supplied")]
    UnusedParameters { used: usize, supplied: usize },
    #[error("placeholder $0 is invalid (positional placeholders are 1-indexed)")]
    ZeroIndexedPlaceholder,
    #[error("empty sql string")]
    EmptySql,
}

pub type PreparedResult<T> = std::result::Result<T, PreparedError>;

impl PreparedStatement {
    pub fn new(sql: impl Into<String>, params: Vec<Value>) -> Self {
        Self { sql: sql.into(), params }
    }

    /// ADR 0139 — syntactic validation of the envelope.
    /// Does NOT consult a catalog or bind parameters to
    /// column types.
    pub fn validate(&self) -> PreparedResult<()> {
        if self.sql.is_empty() {
            return Err(PreparedError::EmptySql);
        }
        let used = scan_placeholders(&self.sql)?;
        for p in &used {
            if (*p as usize) > self.params.len() {
                return Err(PreparedError::PlaceholderOutOfRange {
                    position: *p, supplied: self.params.len(),
                });
            }
        }
        if used.len() != self.params.len() {
            return Err(PreparedError::UnusedParameters {
                used: used.len(), supplied: self.params.len(),
            });
        }
        Ok(())
    }
}

/// Scan `$1`, `$2`, … positional placeholders in the SQL
/// text. Returns the SET of placeholder indexes (not
/// occurrences) — a parameter referenced twice still
/// counts once, which matches typical prepared-statement
/// semantics where each `$N` binds to the same parameter.
fn scan_placeholders(sql: &str) -> PreparedResult<BTreeSet<u32>> {
    let mut out = BTreeSet::new();
    let bytes = sql.as_bytes();
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
            if n == 0 {
                return Err(PreparedError::ZeroIndexedPlaceholder);
            }
            out.insert(n);
            i = j;
        } else {
            i += 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_statement_validates() {
        let p = PreparedStatement::new(
            "SELECT * FROM t WHERE id = $1",
            vec![Value::Int64(42)],
        );
        p.validate().unwrap();
    }

    #[test]
    fn repeated_placeholder_reference_is_ok() {
        let p = PreparedStatement::new(
            "SELECT * FROM t WHERE a = $1 OR b = $1",
            vec![Value::Int64(1)],
        );
        p.validate().unwrap();
    }

    #[test]
    fn multiple_placeholders_in_any_order() {
        let p = PreparedStatement::new(
            "SELECT * FROM t WHERE a = $2 AND b = $1",
            vec![Value::Int64(1), Value::Int64(2)],
        );
        p.validate().unwrap();
    }

    #[test]
    fn placeholder_out_of_range_rejected() {
        let p = PreparedStatement::new(
            "SELECT $5 FROM t",
            vec![Value::Int64(1), Value::Int64(2)],
        );
        assert!(matches!(p.validate(),
            Err(PreparedError::PlaceholderOutOfRange { position: 5, supplied: 2 })));
    }

    #[test]
    fn unused_parameter_rejected() {
        let p = PreparedStatement::new(
            "SELECT $1 FROM t",
            vec![Value::Int64(1), Value::Int64(2)],
        );
        assert!(matches!(p.validate(),
            Err(PreparedError::UnusedParameters { used: 1, supplied: 2 })));
    }

    #[test]
    fn zero_indexed_placeholder_rejected() {
        let p = PreparedStatement::new("SELECT $0 FROM t", vec![Value::Int64(0)]);
        assert!(matches!(p.validate(),
            Err(PreparedError::ZeroIndexedPlaceholder)));
    }

    #[test]
    fn empty_sql_rejected() {
        let p = PreparedStatement::new("", vec![]);
        assert!(matches!(p.validate(), Err(PreparedError::EmptySql)));
    }

    #[test]
    fn no_placeholders_no_params_ok() {
        let p = PreparedStatement::new("SELECT 1", vec![]);
        p.validate().unwrap();
    }

    #[test]
    fn placeholder_followed_by_identifier_boundary() {
        // $1abc — the placeholder parser consumes as many
        // digits as possible, so $1abc is $1 followed by
        // "abc".
        let p = PreparedStatement::new(
            "SELECT $1abc FROM t",
            vec![Value::Int64(1)],
        );
        p.validate().unwrap();
    }

    #[test]
    fn serde_round_trip() {
        let p = PreparedStatement::new(
            "SELECT * FROM t WHERE id = $1",
            vec![Value::Int64(42)],
        );
        let json = serde_json::to_string(&p).unwrap();
        let back: PreparedStatement = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn dollar_without_digit_is_literal() {
        // Treating `$` without a following digit as literal:
        // `$foo` must NOT count as a placeholder.
        let p = PreparedStatement::new("SELECT '$foo' FROM t", vec![]);
        p.validate().unwrap();
    }
}
