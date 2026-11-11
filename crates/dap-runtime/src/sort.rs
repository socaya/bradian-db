//! Sort and top-K operators.
//!
//! The v1 implementation sorts a single batch with the stable scalar `Value::cmp` (IEEE total
//! ordering for f64, NULLs-first). External-memory sort is deferred to a later gate.

use dap_core::{DapError, DapResult, Value};

use crate::batch::{RecordBatch, RecordBatchBuilder};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SortSpec {
    pub column: String,
    pub descending: bool,
    pub nulls_first: bool,
}

/// Sort a batch in place-of-copy and return the sorted batch.
pub fn sort_batch(batch: &RecordBatch, keys: &[SortSpec]) -> DapResult<RecordBatch> {
    let n = batch.row_count();
    let mut idx: Vec<usize> = (0..n).collect();

    let mut key_values: Vec<Vec<Value>> = Vec::with_capacity(keys.len());
    for k in keys {
        let col = batch
            .column(&k.column)
            .map_err(|_| DapError::NotFound(format!("sort column {}", k.column)))?;
        let mut vs = Vec::with_capacity(n);
        for r in 0..n {
            vs.push(col.value_at(r)?);
        }
        key_values.push(vs);
    }

    idx.sort_by(|&a, &b| {
        for (i, k) in keys.iter().enumerate() {
            let va = &key_values[i][a];
            let vb = &key_values[i][b];
            let ord = match (va.is_null(), vb.is_null()) {
                (true, true) => std::cmp::Ordering::Equal,
                // NULL placement is independent of ascending/descending.
                (true, false) => {
                    if k.nulls_first {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Greater
                    }
                }
                (false, true) => {
                    if k.nulls_first {
                        std::cmp::Ordering::Greater
                    } else {
                        std::cmp::Ordering::Less
                    }
                }
                (false, false) => {
                    let o = va.cmp(vb);
                    if k.descending {
                        o.reverse()
                    } else {
                        o
                    }
                }
            };
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        std::cmp::Ordering::Equal
    });

    let mut builder = RecordBatchBuilder::new(batch.schema.clone());
    for i in idx {
        builder.append_row(&batch.row_values(i)?)?;
    }
    builder.finish()
}

#[derive(Debug)]
pub struct TopK {
    k: usize,
    keys: Vec<SortSpec>,
    schema: Option<dap_core::Schema>,
    rows: Vec<Vec<Value>>,
}

impl TopK {
    pub fn new(k: usize, keys: Vec<SortSpec>) -> Self {
        Self {
            k,
            keys,
            schema: None,
            rows: Vec::new(),
        }
    }

    pub fn consume_batch(&mut self, batch: &RecordBatch) -> DapResult<()> {
        if self.schema.is_none() {
            self.schema = Some(batch.schema.clone());
        } else if self.schema.as_ref() != Some(&batch.schema) {
            return Err(DapError::schema_mismatch("top-k schema change"));
        }
        for r in 0..batch.row_count() {
            self.rows.push(batch.row_values(r)?);
        }
        Ok(())
    }

    pub fn finish(self) -> DapResult<RecordBatch> {
        let schema = self.schema.unwrap_or_else(|| {
            dap_core::Schema::new(Vec::new()).expect("empty schema is valid")
        });
        let mut rows = self.rows;
        // Reuse sort_batch by threading through a batch.
        if rows.is_empty() {
            return Ok(RecordBatch::empty(schema));
        }
        let mut builder = RecordBatchBuilder::new(schema.clone());
        for r in &rows {
            builder.append_row(r)?;
        }
        let batch = builder.finish()?;
        let sorted = sort_batch(&batch, &self.keys)?;
        rows.clear();
        let k = self.k.min(sorted.row_count());
        let mut out = RecordBatchBuilder::new(schema);
        for r in 0..k {
            out.append_row(&sorted.row_values(r)?)?;
        }
        out.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dap_core::{Column, LogicalType, Schema};
    use dap_storage::column::ColumnBuilder;

    fn mk_f64_batch(values: &[Option<f64>]) -> RecordBatch {
        let schema = Schema::new(vec![Column::new("v", LogicalType::F64)]).unwrap();
        let mut b = ColumnBuilder::new(LogicalType::F64);
        for v in values {
            match v {
                Some(f) => b.append_value(&Value::F64(*f)).unwrap(),
                None => b.append_value(&Value::Null).unwrap(),
            }
        }
        RecordBatch::new(schema, vec![b.finish()]).unwrap()
    }

    #[test]
    fn ascending_nulls_last() {
        let b = mk_f64_batch(&[Some(3.0), None, Some(1.0), Some(2.0)]);
        let sorted = sort_batch(
            &b,
            &[SortSpec {
                column: "v".into(),
                descending: false,
                nulls_first: false,
            }],
        )
        .unwrap();
        let col = &sorted.columns[0];
        let got: Vec<_> = (0..sorted.row_count())
            .map(|i| col.value_at(i).unwrap())
            .collect();
        assert_eq!(
            got,
            vec![Value::F64(1.0), Value::F64(2.0), Value::F64(3.0), Value::Null]
        );
    }

    #[test]
    fn descending_nulls_first() {
        let b = mk_f64_batch(&[Some(3.0), None, Some(1.0)]);
        let sorted = sort_batch(
            &b,
            &[SortSpec {
                column: "v".into(),
                descending: true,
                nulls_first: true,
            }],
        )
        .unwrap();
        let col = &sorted.columns[0];
        let got: Vec<_> = (0..sorted.row_count())
            .map(|i| col.value_at(i).unwrap())
            .collect();
        assert_eq!(got, vec![Value::Null, Value::F64(3.0), Value::F64(1.0)]);
    }

    #[test]
    fn top_k_returns_bounded_output() {
        let b = mk_f64_batch(&[Some(3.0), Some(1.0), Some(5.0), Some(2.0), Some(4.0)]);
        let mut tk = TopK::new(
            3,
            vec![SortSpec {
                column: "v".into(),
                descending: true,
                nulls_first: false,
            }],
        );
        tk.consume_batch(&b).unwrap();
        let out = tk.finish().unwrap();
        let got: Vec<_> = (0..out.row_count())
            .map(|i| out.column_at(0).value_at(i).unwrap())
            .collect();
        assert_eq!(got, vec![Value::F64(5.0), Value::F64(4.0), Value::F64(3.0)]);
    }
}
