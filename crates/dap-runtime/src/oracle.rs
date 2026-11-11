//! Scalar reference oracle — a slow but obviously-correct implementation of the same logical
//! query shape used by property tests.
//!
//! Spec §5.1: "maintain a scalar reference interpreter as the correctness oracle."

use dap_core::{DapResult, Value};

use crate::aggregate::AggregateKind;
use crate::batch::RecordBatch;
use crate::expr::Predicate;

/// A scalar execution of `WHERE predicate GROUP BY keys AGGREGATE aggs`. Matches the
/// vectorized `scan → filter → hash aggregate` composition.
pub fn aggregate_oracle(
    batches: &[RecordBatch],
    predicate: &Predicate,
    group_by: &[String],
    aggregates: &[AggregateKind],
) -> DapResult<Vec<(Vec<Value>, Vec<Value>)>> {
    let mut groups: std::collections::BTreeMap<Vec<Value>, Vec<Vec<Value>>> =
        std::collections::BTreeMap::new();

    for batch in batches {
        let n = batch.row_count();
        for r in 0..n {
            if !predicate.eval_row(batch, r)? {
                continue;
            }
            let mut key = Vec::with_capacity(group_by.len());
            for g in group_by {
                let idx = batch.schema.index_of(g)?;
                key.push(batch.columns[idx].value_at(r)?);
            }
            let row = batch.row_values(r)?;
            groups.entry(key).or_default().push(row);
        }
    }

    let mut out = Vec::new();
    for (key, rows) in groups {
        let mut agg_values = Vec::with_capacity(aggregates.len());
        for agg in aggregates {
            agg_values.push(scalar_aggregate(agg, &rows, batches)?);
        }
        out.push((key, agg_values));
    }
    Ok(out)
}

fn scalar_aggregate(
    kind: &AggregateKind,
    rows: &[Vec<Value>],
    batches: &[RecordBatch],
) -> DapResult<Value> {
    Ok(match kind {
        AggregateKind::CountStar => Value::I64(rows.len() as i64),
        AggregateKind::CountNonNull { column } => {
            let idx = batches[0].schema.index_of(column)?;
            Value::I64(rows.iter().filter(|r| !r[idx].is_null()).count() as i64)
        }
        AggregateKind::CountDistinct { column } => {
            let idx = batches[0].schema.index_of(column)?;
            let mut set: std::collections::BTreeSet<Value> = Default::default();
            for r in rows {
                let v = &r[idx];
                if !v.is_null() {
                    set.insert(v.clone());
                }
            }
            Value::I64(set.len() as i64)
        }
        AggregateKind::Sum { column } => {
            let idx = batches[0].schema.index_of(column)?;
            let mut sum = 0.0f64;
            let mut saw = false;
            for r in rows {
                if r[idx].is_null() {
                    continue;
                }
                sum += r[idx].as_f64()?;
                saw = true;
            }
            if saw {
                // Preserve integer semantics when input is integral.
                if matches!(rows[0][idx], Value::I64(_)) && sum.fract() == 0.0 {
                    Value::I64(sum as i64)
                } else {
                    Value::F64(sum)
                }
            } else {
                Value::Null
            }
        }
        AggregateKind::Avg { column } => {
            let idx = batches[0].schema.index_of(column)?;
            let (mut sum, mut cnt) = (0.0f64, 0i64);
            for r in rows {
                if r[idx].is_null() {
                    continue;
                }
                sum += r[idx].as_f64()?;
                cnt += 1;
            }
            if cnt == 0 {
                Value::Null
            } else {
                Value::F64(sum / cnt as f64)
            }
        }
        AggregateKind::Min { column } => {
            let idx = batches[0].schema.index_of(column)?;
            rows.iter()
                .map(|r| r[idx].clone())
                .filter(|v| !v.is_null())
                .min()
                .unwrap_or(Value::Null)
        }
        AggregateKind::Max { column } => {
            let idx = batches[0].schema.index_of(column)?;
            rows.iter()
                .map(|r| r[idx].clone())
                .filter(|v| !v.is_null())
                .max()
                .unwrap_or(Value::Null)
        }
    })
}
