//! ADR 0135 — SET NOT NULL backfill validator.
//!
//! BRD-DDL-005 "SET NOT NULL with online backfill" is OPEN.
//! Promoting a nullable column to NOT NULL has two
//! correctness requirements:
//! 1. Every existing row with NULL in that column must be
//!    rewritten with a caller-supplied fill value.
//! 2. The fill value must match the column's declared type.
//!
//! This module ships the pure PRE-FLIGHT validator a future
//! DDL coordinator can call before taking the mutation lock.
//!
//! Honest scope:
//! - Pure function over caller-supplied column + row
//!   iterator + fill value.
//! - Produces a `BackfillPlan` listing the exact row
//!   indices to rewrite and the validated fill value.
//! - Rejects: fill value type mismatch, fill value NULL
//!   (defeats the purpose), unknown column.
//!
//! Out of scope (follow-ups):
//! - Actual row mutation — the coordinator applies the
//!   plan under the RowStore mutation mutex.
//! - Catalog mutator `Catalog::set_not_null(id, col, fill)`
//!   — the mutator itself needs ADR 0126 dependency
//!   coordination, WAL journaling and
//!   HistorySqlEngine reconciliation. Deferred.
//! - Online/concurrent backfill — current scope is
//!   single-threaded, holding the mutation lock.
//! - Backfill with a DEFAULT expression (ADR 0135 ships
//!   literal values only; expressions are part of B1).

use thiserror::Error;

use crate::model::{Column, DataType};
use crate::typed_table::Value;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BackfillError {
    #[error("column {column:?} not found in the row shape")]
    ColumnNotFound { column: String },
    #[error("column {column:?} is already NOT NULL")]
    AlreadyNotNull { column: String },
    #[error("fill value for {column:?} is NULL — would defeat NOT NULL")]
    FillIsNull { column: String },
    #[error("fill value type does not match {column:?} declared type")]
    FillTypeMismatch { column: String },
    #[error("row width does not match the column list")]
    RowWidthMismatch,
}

pub type BackfillResult<T> = std::result::Result<T, BackfillError>;

/// Pre-flight plan for a SET NOT NULL + backfill DDL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackfillPlan {
    pub column: String,
    pub column_index: usize,
    pub fill_value: Value,
    /// Zero-based row indices that currently have NULL in
    /// the target column. Executor rewrites exactly these.
    pub null_row_indices: Vec<usize>,
}

impl BackfillPlan {
    pub fn null_count(&self) -> usize { self.null_row_indices.len() }
    pub fn is_noop(&self) -> bool { self.null_row_indices.is_empty() }
}

/// ADR 0135 — validate a SET NOT NULL + backfill request.
///
/// `columns`: the row schema in positional order (names
/// must match the row-store encoding).
/// `rows`: an iterator over existing rows; each row must
/// have the same width as `columns`.
/// `target_column`: the name of the column being promoted.
/// `fill_value`: the literal that will replace NULLs.
pub fn plan_backfill<'a, I>(
    columns: &[Column],
    rows: I,
    target_column: &str,
    fill_value: Value,
) -> BackfillResult<BackfillPlan>
where
    I: IntoIterator<Item = &'a [Value]>,
{
    let column_index = columns.iter().position(|c| c.name == target_column)
        .ok_or_else(|| BackfillError::ColumnNotFound { column: target_column.into() })?;
    let col = &columns[column_index];
    if !col.nullable {
        return Err(BackfillError::AlreadyNotNull { column: target_column.into() });
    }
    if matches!(fill_value, Value::Null) {
        return Err(BackfillError::FillIsNull { column: target_column.into() });
    }
    if !fill_matches_type(&col.data_type, &fill_value) {
        return Err(BackfillError::FillTypeMismatch { column: target_column.into() });
    }

    let expected_width = columns.len();
    let mut null_row_indices = Vec::new();
    for (i, row) in rows.into_iter().enumerate() {
        if row.len() != expected_width {
            return Err(BackfillError::RowWidthMismatch);
        }
        if matches!(row[column_index], Value::Null) {
            null_row_indices.push(i);
        }
    }

    Ok(BackfillPlan {
        column: target_column.into(),
        column_index,
        fill_value,
        null_row_indices,
    })
}

/// Permissive type match — same contract as
/// `Catalog::set_column_default` (ADR 0124).
fn fill_matches_type(ty: &DataType, v: &Value) -> bool {
    match (ty, v) {
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

    fn cols() -> Vec<Column> {
        vec![
            Column { name: "id".into(),   data_type: DataType::Int64, nullable: false },
            Column { name: "qty".into(),  data_type: DataType::Int32, nullable: true },
            Column { name: "name".into(), data_type: DataType::Text,  nullable: true },
        ]
    }

    fn row(id: i64, qty: Option<i32>, name: Option<&str>) -> Vec<Value> {
        vec![
            Value::Int64(id),
            qty.map_or(Value::Null, Value::Int32),
            name.map_or(Value::Null, |s| Value::String(s.into())),
        ]
    }

    #[test]
    fn plan_picks_up_null_rows_only() {
        let c = cols();
        let rows = [
            row(1, Some(10), Some("a")),
            row(2, None,     Some("b")),
            row(3, Some(20), None),
            row(4, None,     None),
        ];
        let row_refs: Vec<&[Value]> = rows.iter().map(|r| r.as_slice()).collect();
        let plan = plan_backfill(&c, row_refs, "qty", Value::Int32(0)).unwrap();
        assert_eq!(plan.column, "qty");
        assert_eq!(plan.column_index, 1);
        assert_eq!(plan.null_row_indices, vec![1, 3]);
        assert_eq!(plan.null_count(), 2);
        assert!(!plan.is_noop());
    }

    #[test]
    fn no_null_rows_yields_noop_plan() {
        let c = cols();
        let rows = [row(1, Some(10), Some("a")), row(2, Some(20), Some("b"))];
        let row_refs: Vec<&[Value]> = rows.iter().map(|r| r.as_slice()).collect();
        let plan = plan_backfill(&c, row_refs, "qty", Value::Int32(0)).unwrap();
        assert!(plan.is_noop());
        assert_eq!(plan.null_count(), 0);
    }

    #[test]
    fn empty_rows_yields_noop_plan() {
        let c = cols();
        let plan = plan_backfill(&c, Vec::<&[Value]>::new(), "qty", Value::Int32(0)).unwrap();
        assert!(plan.is_noop());
    }

    #[test]
    fn unknown_column_errors() {
        let c = cols();
        let err = plan_backfill(&c, Vec::<&[Value]>::new(), "nope", Value::Int32(0)).unwrap_err();
        assert!(matches!(err, BackfillError::ColumnNotFound { .. }));
    }

    #[test]
    fn already_not_null_errors() {
        let c = cols();
        let err = plan_backfill(&c, Vec::<&[Value]>::new(), "id", Value::Int64(0)).unwrap_err();
        assert!(matches!(err, BackfillError::AlreadyNotNull { .. }));
    }

    #[test]
    fn null_fill_rejected() {
        let c = cols();
        let err = plan_backfill(&c, Vec::<&[Value]>::new(), "qty", Value::Null).unwrap_err();
        assert!(matches!(err, BackfillError::FillIsNull { .. }));
    }

    #[test]
    fn fill_type_mismatch_rejected() {
        let c = cols();
        let err = plan_backfill(&c, Vec::<&[Value]>::new(),
            "qty", Value::String("zero".into())).unwrap_err();
        assert!(matches!(err, BackfillError::FillTypeMismatch { .. }));
    }

    #[test]
    fn row_width_mismatch_rejected() {
        let c = cols();
        let short_row = vec![Value::Int64(1), Value::Null];
        let refs: Vec<&[Value]> = vec![short_row.as_slice()];
        let err = plan_backfill(&c, refs, "qty", Value::Int32(0)).unwrap_err();
        assert!(matches!(err, BackfillError::RowWidthMismatch));
    }

    #[test]
    fn varchar_fill_respects_length_bound() {
        let c = vec![
            Column { name: "s".into(),
                     data_type: DataType::Varchar { max_length: 4 },
                     nullable: true },
        ];
        let refs: Vec<&[Value]> = vec![];
        assert!(plan_backfill(&c, refs.clone(), "s", Value::String("ok".into())).is_ok());
        let err = plan_backfill(&c, refs, "s", Value::String("too long".into())).unwrap_err();
        assert!(matches!(err, BackfillError::FillTypeMismatch { .. }));
    }

    #[test]
    fn json_fill_validates_payload() {
        let c = vec![
            Column { name: "doc".into(), data_type: DataType::Json, nullable: true },
        ];
        let refs: Vec<&[Value]> = vec![];
        assert!(plan_backfill(&c, refs.clone(), "doc",
            Value::String(r#"{"a":1}"#.into())).is_ok());
        let err = plan_backfill(&c, refs, "doc",
            Value::String("not-json".into())).unwrap_err();
        assert!(matches!(err, BackfillError::FillTypeMismatch { .. }));
    }

    #[test]
    fn plan_preserves_fill_value() {
        let c = cols();
        let plan = plan_backfill(&c, Vec::<&[Value]>::new(),
            "qty", Value::Int32(42)).unwrap();
        assert_eq!(plan.fill_value, Value::Int32(42));
    }
}
