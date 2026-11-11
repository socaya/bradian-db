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
    /// Row count that triggers an eager sort + truncate inside `consume_batch`. Keeping
    /// this at `2 * k` means the operator never holds more than ~2k rows at a time
    /// regardless of input size.
    trim_threshold: usize,
}

impl TopK {
    pub fn new(k: usize, keys: Vec<SortSpec>) -> Self {
        let trim_threshold = k.saturating_mul(2).max(1);
        Self {
            k,
            keys,
            schema: None,
            rows: Vec::new(),
            trim_threshold,
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
        if self.rows.len() > self.trim_threshold {
            self.trim_to_top_k()?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> DapResult<RecordBatch> {
        let schema = self.schema.clone().unwrap_or_else(|| {
            dap_core::Schema::new(Vec::new()).expect("empty schema is valid")
        });
        if self.rows.is_empty() {
            return Ok(RecordBatch::empty(schema));
        }
        self.trim_to_top_k()?;
        let mut out = RecordBatchBuilder::new(schema);
        for r in self.rows {
            out.append_row(&r)?;
        }
        out.finish()
    }

    /// Sort the current buffer by the configured keys and truncate down to `k`. Used
    /// both mid-stream (when the buffer grows past `trim_threshold`) and at `finish`.
    fn trim_to_top_k(&mut self) -> DapResult<()> {
        let schema = match self.schema.as_ref() {
            Some(s) => s.clone(),
            None => return Ok(()),
        };
        if self.rows.len() <= self.k {
            return Ok(());
        }
        let mut builder = RecordBatchBuilder::new(schema.clone());
        for r in &self.rows {
            builder.append_row(r)?;
        }
        let batch = builder.finish()?;
        let sorted = sort_batch(&batch, &self.keys)?;
        self.rows.clear();
        let kept = self.k.min(sorted.row_count());
        for i in 0..kept {
            self.rows.push(sorted.row_values(i)?);
        }
        Ok(())
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

    #[test]
    fn top_k_trims_eagerly_under_many_batches() {
        // k = 5 → trim threshold is 10. Feed 10 batches of 10 rows each (100 input rows).
        // The internal buffer must never exceed trim_threshold between batches so memory
        // stays bounded at ~2k regardless of input size.
        let mut tk = TopK::new(
            5,
            vec![SortSpec {
                column: "v".into(),
                descending: true,
                nulls_first: false,
            }],
        );
        for batch_idx in 0..10 {
            let values: Vec<Option<f64>> =
                (0..10).map(|j| Some((batch_idx * 10 + j) as f64)).collect();
            let b = mk_f64_batch(&values);
            tk.consume_batch(&b).unwrap();
            assert!(
                tk.rows.len() <= tk.trim_threshold,
                "buffer size {} must stay <= trim_threshold {}",
                tk.rows.len(),
                tk.trim_threshold,
            );
        }
        let out = tk.finish().unwrap();
        assert_eq!(out.row_count(), 5);
        let got: Vec<_> = (0..out.row_count())
            .map(|i| out.column_at(0).value_at(i).unwrap())
            .collect();
        // Descending: top 5 of 0..100 are 99, 98, 97, 96, 95.
        assert_eq!(
            got,
            vec![
                Value::F64(99.0),
                Value::F64(98.0),
                Value::F64(97.0),
                Value::F64(96.0),
                Value::F64(95.0)
            ]
        );
    }

    #[test]
    fn top_k_with_k_zero_yields_empty() {
        let b = mk_f64_batch(&[Some(1.0), Some(2.0), Some(3.0)]);
        let mut tk = TopK::new(
            0,
            vec![SortSpec {
                column: "v".into(),
                descending: true,
                nulls_first: false,
            }],
        );
        tk.consume_batch(&b).unwrap();
        let out = tk.finish().unwrap();
        assert_eq!(out.row_count(), 0);
    }
}
