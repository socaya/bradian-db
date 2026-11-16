//! ADR 0136 — DEFAULT expression primitive.
//!
//! BRD-DDL-006 "DEFAULT expressions" is OPEN. ADR 0124
//! Catalog::set_column_default stores a literal `Value`
//! only. This module ships a narrow expression enum +
//! evaluator the catalog can store and evaluate at
//! INSERT time.
//!
//! Honest scope — only three expression kinds:
//! - `Literal(Value)` — the existing ADR 0124 behavior.
//! - `CurrentTimestamp` — captured at INSERT time; the
//!   evaluator takes the clock as a parameter so tests
//!   are deterministic.
//! - `NullExpr` — explicit `Value::Null` default (only
//!   valid on nullable columns; validator enforces this).
//!
//! No arithmetic, no function calls, no column refs, no
//! subqueries. All of those are follow-up ADRs.
//!
//! Serialization: the enum round-trips via serde so a
//! future catalog journal can persist it.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::{Column, DataType};
use crate::typed_table::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefaultExpr {
    /// A pre-computed literal. Same contract as ADR 0124
    /// `Catalog::set_column_default`.
    Literal(Value),
    /// Evaluates to the clock reading passed to `eval`.
    /// Only valid on `TimestampTz` columns.
    CurrentTimestamp,
    /// Evaluates to `Value::Null`. Only valid on nullable
    /// columns.
    NullExpr,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DefaultExprError {
    #[error("default literal does not match column {column:?} declared type")]
    LiteralTypeMismatch { column: String },
    #[error("CURRENT_TIMESTAMP default is only valid on TimestampTz columns ({column:?})")]
    CurrentTimestampOnNonTimestamp { column: String },
    #[error("NULL default is only valid on nullable columns ({column:?})")]
    NullDefaultOnNotNull { column: String },
}

pub type DefaultExprResult<T> = std::result::Result<T, DefaultExprError>;

impl DefaultExpr {
    /// ADR 0136 — validate the expression against the
    /// declared column. Pure; no clock needed.
    pub fn validate(&self, column: &Column) -> DefaultExprResult<()> {
        match self {
            DefaultExpr::Literal(v) => {
                if !literal_matches_type(&column.data_type, v) {
                    return Err(DefaultExprError::LiteralTypeMismatch {
                        column: column.name.clone(),
                    });
                }
                // Treat literal NULL the same as NullExpr.
                if matches!(v, Value::Null) && !column.nullable {
                    return Err(DefaultExprError::NullDefaultOnNotNull {
                        column: column.name.clone(),
                    });
                }
                Ok(())
            }
            DefaultExpr::CurrentTimestamp => {
                if !matches!(column.data_type, DataType::TimestampTz) {
                    return Err(DefaultExprError::CurrentTimestampOnNonTimestamp {
                        column: column.name.clone(),
                    });
                }
                Ok(())
            }
            DefaultExpr::NullExpr => {
                if !column.nullable {
                    return Err(DefaultExprError::NullDefaultOnNotNull {
                        column: column.name.clone(),
                    });
                }
                Ok(())
            }
        }
    }

    /// ADR 0136 — evaluate at INSERT time. `now_micros` is
    /// the caller's clock reading (micros since UNIX epoch).
    pub fn eval(&self, now_micros: i64) -> Value {
        match self {
            DefaultExpr::Literal(v) => v.clone(),
            DefaultExpr::CurrentTimestamp => Value::TimestampTz(now_micros),
            DefaultExpr::NullExpr => Value::Null,
        }
    }
}

fn literal_matches_type(ty: &DataType, v: &Value) -> bool {
    match (ty, v) {
        (_, Value::Null) => true,  // nullability checked separately
        (DataType::Boolean, Value::Boolean(_)) => true,
        (DataType::Int32, Value::Int32(_)) => true,
        (DataType::Int64, Value::Int64(_)) => true,
        (DataType::Text, Value::String(_)) => true,
        (DataType::TimestampTz, Value::TimestampTz(_)) => true,
        (DataType::Uuid, Value::Uuid(_)) => true,
        (DataType::Varchar { max_length }, Value::String(s)) =>
            s.len() <= *max_length as usize,
        (DataType::Decimal { precision, scale }, Value::Decimal(d)) => {
            if *precision == 0 || *precision > 38 || *scale > *precision { false }
            else {
                let mag = d.unsigned_abs();
                mag < 10_u128.pow(*precision as u32)
            }
        }
        (DataType::Json, Value::String(s)) =>
            serde_json::from_str::<serde_json::Value>(s).is_ok(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, dt: DataType, nullable: bool) -> Column {
        Column { name: name.into(), data_type: dt, nullable }
    }

    #[test]
    fn literal_default_validates_and_evaluates() {
        let c = col("x", DataType::Int32, false);
        let e = DefaultExpr::Literal(Value::Int32(42));
        e.validate(&c).unwrap();
        assert_eq!(e.eval(0), Value::Int32(42));
    }

    #[test]
    fn literal_type_mismatch_rejected() {
        let c = col("x", DataType::Int32, false);
        let e = DefaultExpr::Literal(Value::String("nope".into()));
        assert!(matches!(e.validate(&c),
            Err(DefaultExprError::LiteralTypeMismatch { .. })));
    }

    #[test]
    fn literal_null_on_not_null_rejected() {
        let c = col("x", DataType::Int32, false);
        let e = DefaultExpr::Literal(Value::Null);
        assert!(matches!(e.validate(&c),
            Err(DefaultExprError::NullDefaultOnNotNull { .. })));
    }

    #[test]
    fn literal_null_on_nullable_accepted() {
        let c = col("x", DataType::Int32, true);
        let e = DefaultExpr::Literal(Value::Null);
        e.validate(&c).unwrap();
        assert_eq!(e.eval(0), Value::Null);
    }

    #[test]
    fn current_timestamp_on_timestamptz_evaluates_to_clock() {
        let c = col("ts", DataType::TimestampTz, false);
        let e = DefaultExpr::CurrentTimestamp;
        e.validate(&c).unwrap();
        assert_eq!(e.eval(1_700_000_000_000_000), Value::TimestampTz(1_700_000_000_000_000));
    }

    #[test]
    fn current_timestamp_on_non_timestamp_rejected() {
        let c = col("x", DataType::Int64, false);
        let e = DefaultExpr::CurrentTimestamp;
        assert!(matches!(e.validate(&c),
            Err(DefaultExprError::CurrentTimestampOnNonTimestamp { .. })));
    }

    #[test]
    fn null_expr_only_on_nullable() {
        let nullable = col("x", DataType::Int32, true);
        DefaultExpr::NullExpr.validate(&nullable).unwrap();
        let not_null = col("y", DataType::Int32, false);
        assert!(matches!(DefaultExpr::NullExpr.validate(&not_null),
            Err(DefaultExprError::NullDefaultOnNotNull { .. })));
    }

    #[test]
    fn null_expr_evaluates_to_null() {
        assert_eq!(DefaultExpr::NullExpr.eval(42), Value::Null);
    }

    #[test]
    fn default_expr_round_trips_through_serde_json() {
        for e in [
            DefaultExpr::Literal(Value::Int32(42)),
            DefaultExpr::CurrentTimestamp,
            DefaultExpr::NullExpr,
        ] {
            let s = serde_json::to_string(&e).unwrap();
            let back: DefaultExpr = serde_json::from_str(&s).unwrap();
            assert_eq!(e, back);
        }
    }

    #[test]
    fn literal_varchar_length_respected() {
        let c = col("s", DataType::Varchar { max_length: 4 }, false);
        DefaultExpr::Literal(Value::String("ok".into())).validate(&c).unwrap();
        assert!(matches!(
            DefaultExpr::Literal(Value::String("too long".into())).validate(&c),
            Err(DefaultExprError::LiteralTypeMismatch { .. })));
    }

    #[test]
    fn literal_json_content_validated() {
        let c = col("doc", DataType::Json, false);
        DefaultExpr::Literal(Value::String(r#"{"a":1}"#.into())).validate(&c).unwrap();
        assert!(matches!(
            DefaultExpr::Literal(Value::String("not-json".into())).validate(&c),
            Err(DefaultExprError::LiteralTypeMismatch { .. })));
    }
}
