//! ADR 0133 — row ↔ column batch converter.
//!
//! BRD-HTAP-003 "vectorized columnar reads" is OPEN. This
//! module ships the correctness PRIMITIVE a future
//! vectorized reader path can call: given a batch of typed
//! rows, produce column-oriented buffers; given columns,
//! rebuild the row batch.
//!
//! Honest scope:
//! - Pure data transformation — no I/O, no execution.
//! - Preserves per-column `DataType` metadata so callers
//!   can validate that output columns match the declared
//!   schema.
//! - Round-trips bit-exact.
//!
//! Out of scope (follow-ups):
//! - **Engine-level vectorized execution** — SQL engines
//!   still return `Vec<Vec<Value>>`. Wiring a batch-oriented
//!   row iterator into each engine is per-engine ADR work.
//! - **SIMD / arithmetic over column buffers** — this
//!   primitive is layout-only.
//! - **Null bitmap compaction** — nulls are stored as
//!   `Value::Null` entries in the same `Vec<Value>`.
//!   Future ADR can add a dense null bitmap + packed
//!   value buffer per dtype.
//! - **Dictionary encoding / RLE / segment file writer**
//!   — ADR 0082 already covers DictIndex32; this
//!   primitive is the uncompressed stepping stone.

use thiserror::Error;

use crate::model::DataType;
use crate::typed_table::Value;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BatchError {
    #[error("row width {row_width} does not match column count {expected}")]
    RowWidthMismatch { row_width: usize, expected: usize },
    #[error("column length {found} does not match batch height {expected}")]
    ColumnHeightMismatch { expected: usize, found: usize },
    #[error("column list is empty")]
    EmptyColumnList,
    #[error("duplicate column name {0:?}")]
    DuplicateColumnName(String),
}

pub type BatchResult<T> = std::result::Result<T, BatchError>;

/// One column of a batch. `dtype` carries the declared
/// type for validation; `values` has one entry per row,
/// with `Value::Null` representing SQL NULL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnBatch {
    pub name: String,
    pub dtype: DataType,
    pub values: Vec<Value>,
}

/// Row-oriented batch: a shared column order + a Vec of
/// equal-width rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowBatch {
    pub columns: Vec<(String, DataType)>,
    pub rows: Vec<Vec<Value>>,
}

impl RowBatch {
    pub fn height(&self) -> usize { self.rows.len() }
    pub fn width(&self) -> usize { self.columns.len() }
}

/// ADR 0133 — convert a row-oriented batch into column-
/// oriented buffers. Validates every row is the declared
/// width.
pub fn rows_to_columns(batch: &RowBatch) -> BatchResult<Vec<ColumnBatch>> {
    if batch.columns.is_empty() {
        return Err(BatchError::EmptyColumnList);
    }
    let expected = batch.columns.len();
    for (ri, row) in batch.rows.iter().enumerate() {
        if row.len() != expected {
            return Err(BatchError::RowWidthMismatch {
                row_width: row.len(),
                expected,
            });
        }
        // Reject duplicate column names once (first row only
        // for cost reasons — the column list is shared).
        if ri == 0 {
            let mut seen = std::collections::BTreeSet::new();
            for (name, _) in &batch.columns {
                if !seen.insert(name.clone()) {
                    return Err(BatchError::DuplicateColumnName(name.clone()));
                }
            }
        }
    }
    let mut out: Vec<ColumnBatch> = batch.columns.iter().map(|(n, t)| ColumnBatch {
        name: n.clone(), dtype: t.clone(),
        values: Vec::with_capacity(batch.rows.len()),
    }).collect();
    for row in &batch.rows {
        for (ci, v) in row.iter().enumerate() {
            out[ci].values.push(v.clone());
        }
    }
    Ok(out)
}

/// ADR 0133 — rebuild a `RowBatch` from column buffers.
/// All columns must have identical length.
pub fn columns_to_rows(cols: &[ColumnBatch]) -> BatchResult<RowBatch> {
    if cols.is_empty() {
        return Err(BatchError::EmptyColumnList);
    }
    let height = cols[0].values.len();
    for c in cols.iter() {
        if c.values.len() != height {
            return Err(BatchError::ColumnHeightMismatch {
                expected: height, found: c.values.len(),
            });
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for c in cols {
        if !seen.insert(c.name.clone()) {
            return Err(BatchError::DuplicateColumnName(c.name.clone()));
        }
    }
    let columns: Vec<(String, DataType)> = cols.iter()
        .map(|c| (c.name.clone(), c.dtype.clone())).collect();
    let mut rows = Vec::with_capacity(height);
    for r in 0..height {
        let row: Vec<Value> = cols.iter().map(|c| c.values[r].clone()).collect();
        rows.push(row);
    }
    Ok(RowBatch { columns, rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_batch() -> RowBatch {
        RowBatch {
            columns: vec![
                ("id".into(), DataType::Int64),
                ("name".into(), DataType::Text),
                ("active".into(), DataType::Boolean),
            ],
            rows: vec![
                vec![Value::Int64(1), Value::String("alice".into()), Value::Boolean(true)],
                vec![Value::Int64(2), Value::String("bob".into()),   Value::Boolean(false)],
                vec![Value::Int64(3), Value::Null,                   Value::Boolean(true)],
            ],
        }
    }

    #[test]
    fn rows_to_columns_preserves_dtype_and_order() {
        let cols = rows_to_columns(&make_batch()).unwrap();
        assert_eq!(cols.len(), 3);
        assert_eq!(cols[0].name, "id");
        assert_eq!(cols[0].dtype, DataType::Int64);
        assert_eq!(cols[0].values, vec![Value::Int64(1), Value::Int64(2), Value::Int64(3)]);
        assert_eq!(cols[1].values[2], Value::Null);
    }

    #[test]
    fn round_trip_is_bit_exact() {
        let b = make_batch();
        let cols = rows_to_columns(&b).unwrap();
        let b2 = columns_to_rows(&cols).unwrap();
        assert_eq!(b, b2);
    }

    #[test]
    fn empty_rows_round_trips() {
        let b = RowBatch {
            columns: vec![("a".into(), DataType::Int32)],
            rows: vec![],
        };
        let cols = rows_to_columns(&b).unwrap();
        assert_eq!(cols[0].values.len(), 0);
        let b2 = columns_to_rows(&cols).unwrap();
        assert_eq!(b, b2);
    }

    #[test]
    fn row_width_mismatch_errors() {
        let mut b = make_batch();
        b.rows[1].pop(); // too short
        let err = rows_to_columns(&b).unwrap_err();
        assert!(matches!(err, BatchError::RowWidthMismatch { .. }));
    }

    #[test]
    fn empty_column_list_errors() {
        let b = RowBatch { columns: vec![], rows: vec![] };
        let err = rows_to_columns(&b).unwrap_err();
        assert_eq!(err, BatchError::EmptyColumnList);
        let err = columns_to_rows(&[]).unwrap_err();
        assert_eq!(err, BatchError::EmptyColumnList);
    }

    #[test]
    fn duplicate_column_names_errors() {
        let b = RowBatch {
            columns: vec![
                ("x".into(), DataType::Int32),
                ("x".into(), DataType::Int32),
            ],
            rows: vec![vec![Value::Int32(1), Value::Int32(2)]],
        };
        let err = rows_to_columns(&b).unwrap_err();
        assert!(matches!(err, BatchError::DuplicateColumnName(_)));
    }

    #[test]
    fn column_height_mismatch_errors() {
        let cols = vec![
            ColumnBatch { name: "a".into(), dtype: DataType::Int32,
                          values: vec![Value::Int32(1), Value::Int32(2)] },
            ColumnBatch { name: "b".into(), dtype: DataType::Int32,
                          values: vec![Value::Int32(1)] },
        ];
        let err = columns_to_rows(&cols).unwrap_err();
        assert!(matches!(err, BatchError::ColumnHeightMismatch { .. }));
    }

    #[test]
    fn columns_to_rows_duplicate_name_errors() {
        let cols = vec![
            ColumnBatch { name: "x".into(), dtype: DataType::Int32,
                          values: vec![Value::Int32(1)] },
            ColumnBatch { name: "x".into(), dtype: DataType::Int32,
                          values: vec![Value::Int32(2)] },
        ];
        let err = columns_to_rows(&cols).unwrap_err();
        assert!(matches!(err, BatchError::DuplicateColumnName(_)));
    }

    #[test]
    fn height_width_accessors() {
        let b = make_batch();
        assert_eq!(b.height(), 3);
        assert_eq!(b.width(), 3);
    }

    #[test]
    fn column_batch_values_preserve_nulls() {
        let b = make_batch();
        let cols = rows_to_columns(&b).unwrap();
        let nulls = cols[1].values.iter().filter(|v| matches!(v, Value::Null)).count();
        assert_eq!(nulls, 1);
    }
}
