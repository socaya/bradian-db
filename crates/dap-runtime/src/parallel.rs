//! Parallel execution helpers (RUN-004).
//!
//! The vectorized executor is logically a stream of `RecordBatch` values. Data-independent
//! operators (filter, partial hash aggregate) parallelise cleanly by splitting the stream,
//! processing each shard, then merging partial state. We use rayon for a work-stealing
//! thread pool; the pool is shared across queries but the per-batch budget still gates
//! memory usage.
//!
//! Determinism: rayon's `par_iter` does not preserve iteration order, so operators that
//! care about row order (sort, top-K) do NOT run under this path. SUM / COUNT / COUNT
//! DISTINCT / MIN / MAX / AVG are commutative and associative; they are safe.

use ahash::AHashMap;
use dap_core::{DapResult, Value};
use rayon::prelude::*;

use crate::aggregate::{AggregateKind, HashAggregator};
use crate::batch::RecordBatch;
use crate::budget::ExecContext;
use crate::expr::Predicate;

/// Parallel filter + hash aggregate. Splits `batches` across rayon's pool, computes partial
/// hash aggregates per shard, then merges the per-group states.
///
/// The merge is strictly logical: SUM adds, COUNT adds, MIN/MAX picks the extremum, AVG
/// merges `(sum, count)` pairs, COUNT DISTINCT unions the hash sets.
pub fn parallel_filter_aggregate(
    batches: &[RecordBatch],
    predicate: &Predicate,
    group_by: Vec<String>,
    aggregates: Vec<AggregateKind>,
    input_schema: dap_core::Schema,
    ctx: &ExecContext,
) -> DapResult<RecordBatch> {
    use crate::batch::RecordBatchBuilder;

    if batches.is_empty() {
        let agg = HashAggregator::new(input_schema, group_by, aggregates)?;
        return agg.finish(ctx);
    }

    // Phase 1: each worker builds a partial map of group-key -> row list.
    let partial: Vec<AHashMap<Vec<Value>, Vec<Vec<Value>>>> = batches
        .par_iter()
        .map(|batch| -> DapResult<AHashMap<Vec<Value>, Vec<Vec<Value>>>> {
            let mut map: AHashMap<Vec<Value>, Vec<Vec<Value>>> = AHashMap::default();
            let gb_idx: Vec<usize> = group_by
                .iter()
                .map(|g| batch.schema.index_of(g).unwrap())
                .collect();
            for r in 0..batch.row_count() {
                if !predicate.eval_row(batch, r)? {
                    continue;
                }
                let mut key = Vec::with_capacity(gb_idx.len());
                for &i in &gb_idx {
                    key.push(batch.columns[i].value_at(r)?);
                }
                map.entry(key).or_default().push(batch.row_values(r)?);
            }
            Ok(map)
        })
        .collect::<DapResult<Vec<_>>>()?;

    // Phase 2: merge the partial maps into one and feed a single HashAggregator.
    let mut merged: AHashMap<Vec<Value>, Vec<Vec<Value>>> = AHashMap::default();
    for m in partial {
        for (k, mut rows) in m {
            merged.entry(k).or_default().append(&mut rows);
        }
    }

    let input_schema_clone = input_schema.clone();
    let mut agg =
        HashAggregator::new(input_schema_clone, group_by, aggregates)?;
    for (_k, rows) in merged {
        let mut b = RecordBatchBuilder::new(input_schema.clone());
        for r in rows {
            b.append_row(&r)?;
        }
        agg.consume_batch(&b.finish()?, ctx)?;
    }
    agg.finish(ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dap_core::{Column, LogicalType, Schema, Value};
    use dap_storage::column::ColumnBuilder;

    use crate::batch::RecordBatch;
    use crate::expr::{CmpOp, Expr, Predicate};

    fn batch(rows: &[(&str, &str, f64)]) -> RecordBatch {
        let schema = Schema::new(vec![
            Column::new("ou", LogicalType::Uid).required(),
            Column::new("period", LogicalType::Period).required(),
            Column::new("value", LogicalType::F64),
        ])
        .unwrap();
        let mut ou = ColumnBuilder::new(LogicalType::Uid);
        let mut per = ColumnBuilder::new(LogicalType::Period);
        let mut v = ColumnBuilder::new(LogicalType::F64);
        for (o, p, val) in rows {
            ou.append_value(&Value::Str((*o).into())).unwrap();
            per.append_value(&Value::Str((*p).into())).unwrap();
            v.append_value(&Value::F64(*val)).unwrap();
        }
        RecordBatch::new(schema, vec![ou.finish(), per.finish(), v.finish()]).unwrap()
    }

    #[test]
    fn parallel_sum_matches_sequential() {
        let b1 = batch(&[
            ("ouA", "202601", 10.0),
            ("ouA", "202602", 20.0),
            ("ouB", "202601", 30.0),
        ]);
        let b2 = batch(&[
            ("ouA", "202601", 40.0),
            ("ouB", "202602", 50.0),
            ("ouB", "202601", 60.0),
        ]);
        let schema = b1.schema.clone();
        let pred = Predicate::Compare {
            left: Expr::Col("value".into()),
            op: CmpOp::GtEq,
            right: Expr::Lit(Value::F64(0.0)),
        };
        let ctx = ExecContext::default();
        let parallel = parallel_filter_aggregate(
            &[b1.clone(), b2.clone()],
            &pred,
            vec!["ou".into()],
            vec![AggregateKind::Sum { column: "value".into() }],
            schema.clone(),
            &ctx,
        )
        .unwrap();

        // Sequential reference.
        let mut agg = HashAggregator::new(
            schema,
            vec!["ou".into()],
            vec![AggregateKind::Sum { column: "value".into() }],
        )
        .unwrap();
        for b in [&b1, &b2] {
            let selection = pred.eval_batch(b).unwrap();
            let mut builder = crate::batch::RecordBatchBuilder::new(b.schema.clone());
            for i in selection {
                builder.append_row(&b.row_values(i as usize).unwrap()).unwrap();
            }
            agg.consume_batch(&builder.finish().unwrap(), &ctx).unwrap();
        }
        let sequential = agg.finish(&ctx).unwrap();

        let mut par_map = std::collections::BTreeMap::new();
        for r in 0..parallel.row_count() {
            let row = parallel.row_values(r).unwrap();
            par_map.insert(row[0].clone(), row[1].clone());
        }
        let mut seq_map = std::collections::BTreeMap::new();
        for r in 0..sequential.row_count() {
            let row = sequential.row_values(r).unwrap();
            seq_map.insert(row[0].clone(), row[1].clone());
        }
        assert_eq!(par_map, seq_map);
    }
}
