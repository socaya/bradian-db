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

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::typed_table::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedStatement {
    pub sql: String,
    pub params: Vec<Value>,
    /// ADR 0183 — named parameters referenced as `:name` in
    /// the SQL template. Both positional (`$N`) and named
    /// (`:name`) placeholders may coexist in one template.
    /// Empty map preserves pre-ADR-0183 wire bytes
    /// (`#[serde(default, skip_serializing_if = empty)]`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub named_params: BTreeMap<String, Value>,
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
    /// ADR 0183 — named-parameter reference with no binding.
    #[error("sql references named parameter :{name} but no binding was supplied")]
    UnknownNamedParameter { name: String },
    /// ADR 0183 — named-parameter binding not referenced.
    #[error("named parameter :{name} was supplied but not referenced in sql")]
    UnusedNamedParameter { name: String },
    /// ADR 0183 — rejected name for a parameter (must match
    /// `^[A-Za-z_][A-Za-z0-9_]*`).
    #[error("invalid named parameter identifier :{name:?}")]
    InvalidNamedParameter { name: String },
}

pub type PreparedResult<T> = std::result::Result<T, PreparedError>;

impl PreparedStatement {
    pub fn new(sql: impl Into<String>, params: Vec<Value>) -> Self {
        Self { sql: sql.into(), params, named_params: BTreeMap::new() }
    }

    /// ADR 0183 — constructor for a statement with named
    /// parameters only. Positional and named may coexist;
    /// callers who want both use the struct literal.
    pub fn with_named(
        sql: impl Into<String>, named_params: BTreeMap<String, Value>,
    ) -> Self {
        Self { sql: sql.into(), params: Vec::new(), named_params }
    }

    /// ADR 0139 — syntactic validation of the envelope.
    /// ADR 0183 — extended to validate named parameters:
    /// every `:name` must have a binding; every binding
    /// must be referenced; parameter names must be ASCII
    /// identifiers. Does NOT consult a catalog or bind
    /// parameters to column types.
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
        for name in self.named_params.keys() {
            if !is_valid_named_identifier(name) {
                return Err(PreparedError::InvalidNamedParameter {
                    name: name.clone(),
                });
            }
        }
        let used_named = scan_named_placeholders(&self.sql);
        for name in &used_named {
            if !self.named_params.contains_key(name) {
                return Err(PreparedError::UnknownNamedParameter {
                    name: name.clone(),
                });
            }
        }
        for name in self.named_params.keys() {
            if !used_named.contains(name) {
                return Err(PreparedError::UnusedNamedParameter {
                    name: name.clone(),
                });
            }
        }
        Ok(())
    }
}

fn is_valid_named_identifier(s: &str) -> bool {
    let bytes = s.as_bytes();
    !bytes.is_empty()
        && (bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        && bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

/// ADR 0183 — scan `:name` named placeholders. Names are
/// ASCII identifiers (start with letter/underscore, then
/// alphanumeric/underscore). Does not cross string-literal
/// boundaries at this layer — same honest limitation
/// documented for positional `$N` scanning.
pub fn scan_named_placeholders(sql: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let bytes = sql.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' && i + 1 < bytes.len()
            && (bytes[i + 1].is_ascii_alphabetic() || bytes[i + 1] == b'_')
        {
            let mut j = i + 1;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_')
            {
                j += 1;
            }
            out.insert(
                std::str::from_utf8(&bytes[i + 1..j])
                    .expect("ascii identifier is valid utf8")
                    .to_string(),
            );
            i = j;
        } else {
            i += 1;
        }
    }
    out
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
