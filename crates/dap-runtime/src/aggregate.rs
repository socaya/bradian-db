//! Hash aggregate.
//!
//! Supports SUM, COUNT (star + non-null), MIN, MAX, AVG (as mergeable (sum, count)),
//! and COUNT_DISTINCT (exact, reference implementation). Group-by is over any set of columns.
//!
//! The implementation is correctness-first: it goes through the scalar `Value` oracle for the
//! GROUP BY key and the measure values. A specialised typed kernel can be added once we have
//! real workload profiles; the scalar implementation remains the reference oracle.

use ahash::AHashMap;
use dap_core::{Column, DapError, DapResult, LogicalType, Schema, Value};

use crate::batch::{RecordBatch, RecordBatchBuilder};
use crate::budget::ExecContext;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AggregateKind {
    CountStar,
    CountNonNull { column: String },
    CountDistinct { column: String },
    Sum { column: String },
    Min { column: String },
    Max { column: String },
    Avg { column: String },
}

impl AggregateKind {
    pub fn output_name(&self) -> String {
        match self {
            AggregateKind::CountStar => "count".into(),
            AggregateKind::CountNonNull { column } => format!("count_{column}"),
            AggregateKind::CountDistinct { column } => format!("count_distinct_{column}"),
            AggregateKind::Sum { column } => format!("sum_{column}"),
            AggregateKind::Min { column } => format!("min_{column}"),
            AggregateKind::Max { column } => format!("max_{column}"),
            AggregateKind::Avg { column } => format!("avg_{column}"),
        }
    }

    pub fn output_type(&self, input_schema: &Schema) -> DapResult<LogicalType> {
        Ok(match self {
            AggregateKind::CountStar
            | AggregateKind::CountNonNull { .. }
            | AggregateKind::CountDistinct { .. } => LogicalType::I64,
            AggregateKind::Sum { column }
            | AggregateKind::Min { column }
            | AggregateKind::Max { column } => input_schema.column(column)?.logical_type,
            AggregateKind::Avg { .. } => LogicalType::F64,
        })
    }
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
enum AggState {
    #[default]
    None,
    Count(i64),
    SumI64 {
        sum: i128,
        count: i64,
    },
    SumF64 {
        sum: f64,
        count: i64,
    },
    Min(Value),
    Max(Value),
    CountDistinct(ahash::AHashSet<Value>),
}

impl AggState {
    fn ingest(&mut self, v: &Value) {
        match self {
            AggState::Count(c) => *c += 1,
            AggState::SumI64 { sum, count } => {
                if let Value::I64(i) = v {
                    *sum += *i as i128;
                    *count += 1;
                }
            }
            AggState::SumF64 { sum, count } => {
                if let Ok(f) = v.as_f64() {
                    *sum += f;
                    *count += 1;
                }
            }
            AggState::Min(acc) => {
                if acc.is_null() || v < acc {
                    *acc = v.clone();
                }
            }
            AggState::Max(acc) => {
                if acc.is_null() || v > acc {
                    *acc = v.clone();
                }
            }
            AggState::CountDistinct(set) => {
                set.insert(v.clone());
            }
            AggState::None => {}
        }
    }

    fn finish(self, kind: &AggregateKind) -> Value {
        match (self, kind) {
            (AggState::Count(c), _) => Value::I64(c),
            (AggState::SumI64 { sum, .. }, AggregateKind::Sum { .. }) => {
                Value::I64(sum as i64)
            }
            (AggState::SumF64 { sum, .. }, AggregateKind::Sum { .. }) => Value::F64(sum),
            (AggState::SumI64 { sum, count }, AggregateKind::Avg { .. }) => {
                if count == 0 {
                    Value::Null
                } else {
                    Value::F64((sum as f64) / (count as f64))
                }
            }
            (AggState::SumF64 { sum, count }, AggregateKind::Avg { .. }) => {
                if count == 0 {
                    Value::Null
                } else {
                    Value::F64(sum / count as f64)
                }
            }
            (AggState::Min(v), _) | (AggState::Max(v), _) => v,
            (AggState::CountDistinct(set), _) => Value::I64(set.len() as i64),
            (AggState::None, _) => Value::Null,
            // Fall-through for state/kind combinations that should not arise because
            // `new_state` picks the right state for each kind. If we hit it, something upstream
            // paired the wrong aggregate with the wrong state.
            (_, _) => Value::Null,
        }
    }
}

fn new_state(kind: &AggregateKind, logical: LogicalType) -> AggState {
    match kind {
        AggregateKind::CountStar | AggregateKind::CountNonNull { .. } => AggState::Count(0),
        AggregateKind::Sum { .. } | AggregateKind::Avg { .. } => match logical {
            LogicalType::I64 | LogicalType::I32 => AggState::SumI64 { sum: 0, count: 0 },
            _ => AggState::SumF64 { sum: 0.0, count: 0 },
        },
        AggregateKind::Min { .. } => AggState::Min(Value::Null),
        AggregateKind::Max { .. } => AggState::Max(Value::Null),
        AggregateKind::CountDistinct { .. } => AggState::CountDistinct(Default::default()),
    }
}

pub struct HashAggregator {
    group_by: Vec<String>,
    aggregates: Vec<AggregateKind>,
    input_schema: Schema,
    output_schema: Schema,
    groups: AHashMap<Vec<Value>, Vec<AggState>>,
    /// Paths of spilled partial-state files. Each file is a bincode-encoded vector of
    /// `(key, Vec<AggState>)` pairs. Merged in `finish` before returning the final batch.
    spilled: Vec<std::path::PathBuf>,
}

impl HashAggregator {
    pub fn new(
        input_schema: Schema,
        group_by: Vec<String>,
        aggregates: Vec<AggregateKind>,
    ) -> DapResult<Self> {
        // Validate that every group-by + aggregate reference exists.
        for g in &group_by {
            input_schema.column(g)?;
        }
        for a in &aggregates {
            match a {
                AggregateKind::CountStar => {}
                AggregateKind::CountNonNull { column }
                | AggregateKind::CountDistinct { column }
                | AggregateKind::Sum { column }
                | AggregateKind::Min { column }
                | AggregateKind::Max { column }
                | AggregateKind::Avg { column } => {
                    input_schema.column(column)?;
                }
            }
        }
        let mut cols = Vec::new();
        for g in &group_by {
            cols.push(input_schema.column(g)?.clone());
        }
        for a in &aggregates {
            cols.push(Column::new(a.output_name(), a.output_type(&input_schema)?));
        }
        let output_schema = Schema::new(cols)?;
        Ok(Self {
            group_by,
            aggregates,
            input_schema,
            output_schema,
            groups: AHashMap::default(),
            spilled: Vec::new(),
        })
    }

    fn spill_current_groups(&mut self) -> DapResult<()> {
        if self.groups.is_empty() {
            return Ok(());
        }
        let payload: Vec<(Vec<Value>, Vec<AggState>)> =
            self.groups.drain().collect();
        let encoded = bincode::serialize(&payload)
            .map_err(|e| DapError::Serialization(e.to_string()))?;
        // Use a NamedTempFile that persists on disk until this struct is dropped.
        let tmp = tempfile::NamedTempFile::new()?;
        std::fs::write(tmp.path(), &encoded)?;
        self.spilled.push(
            tmp.into_temp_path()
                .keep()
                .map_err(|e| DapError::Io(std::io::Error::other(e)))?,
        );
        Ok(())
    }

    fn drain_spilled(&mut self) -> DapResult<Vec<(Vec<Value>, Vec<AggState>)>> {
        let mut out = Vec::new();
        for path in std::mem::take(&mut self.spilled) {
            let bytes = std::fs::read(&path)?;
            let decoded: Vec<(Vec<Value>, Vec<AggState>)> = bincode::deserialize(&bytes)
                .map_err(|e| DapError::Serialization(e.to_string()))?;
            out.extend(decoded);
            let _ = std::fs::remove_file(&path);
        }
        Ok(out)
    }

    pub fn output_schema(&self) -> &Schema {
        &self.output_schema
    }

    pub fn consume_batch(&mut self, batch: &RecordBatch, ctx: &ExecContext) -> DapResult<()> {
        if batch.schema != self.input_schema {
            return Err(DapError::schema_mismatch(
                "aggregator input schema mismatch",
            ));
        }
        // Spill check: if we're over the memory budget, flush the current hash table to a
        // temp file and clear it. A later `finish` pass will merge all spill files. For v1
        // we spill only when `ExecContext::memory` has a non-zero hard_limit and the
        // in-memory map exceeds the spill_threshold estimate.
        if ctx.memory.hard_limit > 0
            && self.groups.len() as u64 * 128 > ctx.memory.spill_threshold
        {
            self.spill_current_groups()?;
        }
        let gb_indexes: Vec<usize> = self
            .group_by
            .iter()
            .map(|g| self.input_schema.index_of(g).unwrap())
            .collect();

        let n = batch.row_count();
        for r in 0..n {
            ctx.check_cancellation()?;
            let mut key = Vec::with_capacity(gb_indexes.len());
            for &i in &gb_indexes {
                key.push(batch.columns[i].value_at(r)?);
            }
            let states = self.groups.entry(key).or_insert_with(|| {
                self.aggregates
                    .iter()
                    .map(|a| {
                        let lt = match a {
                            AggregateKind::CountStar => LogicalType::I64,
                            AggregateKind::CountNonNull { column }
                            | AggregateKind::CountDistinct { column }
                            | AggregateKind::Sum { column }
                            | AggregateKind::Min { column }
                            | AggregateKind::Max { column }
                            | AggregateKind::Avg { column } => {
                                self.input_schema.column(column).unwrap().logical_type
                            }
                        };
                        new_state(a, lt)
                    })
                    .collect()
            });
            for (i, agg) in self.aggregates.iter().enumerate() {
                let state = &mut states[i];
                match agg {
                    AggregateKind::CountStar => state.ingest(&Value::I64(0)),
                    AggregateKind::CountNonNull { column }
                    | AggregateKind::Sum { column }
                    | AggregateKind::Min { column }
                    | AggregateKind::Max { column }
                    | AggregateKind::Avg { column }
                    | AggregateKind::CountDistinct { column } => {
                        let idx = self.input_schema.index_of(column).unwrap();
                        let v = batch.columns[idx].value_at(r)?;
                        if !v.is_null() {
                            state.ingest(&v);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn finish(mut self, ctx: &ExecContext) -> DapResult<RecordBatch> {
        // Merge any spilled partial states back into the live map before finalising.
        if !self.spilled.is_empty() {
            let spilled_rows = self.drain_spilled()?;
            for (key, states) in spilled_rows {
                match self.groups.get_mut(&key) {
                    Some(existing) => merge_states_in_place(existing, states),
                    None => {
                        self.groups.insert(key, states);
                    }
                }
            }
            ctx.metrics
                .spilled_bytes
                .fetch_add(0, std::sync::atomic::Ordering::Relaxed);
        }

        let mut builder = RecordBatchBuilder::new(self.output_schema.clone());
        for (key, states) in self.groups.into_iter() {
            ctx.check_cancellation()?;
            let mut row = Vec::with_capacity(key.len() + states.len());
            row.extend(key);
            for (s, a) in states.into_iter().zip(self.aggregates.iter()) {
                row.push(s.finish(a));
            }
            builder.append_row(&row)?;
        }
        let batch = builder.finish()?;
        ctx.metrics.record_output(batch.row_count() as u64);
        Ok(batch)
    }
}

fn merge_states_in_place(existing: &mut [AggState], incoming: Vec<AggState>) {
    for (dst, src) in existing.iter_mut().zip(incoming.into_iter()) {
        merge_pair(dst, src);
    }
}

fn merge_pair(dst: &mut AggState, src: AggState) {
    match (std::mem::take(dst), src) {
        (AggState::Count(a), AggState::Count(b)) => {
            *dst = AggState::Count(a + b);
        }
        (AggState::SumI64 { sum: a, count: ca }, AggState::SumI64 { sum: b, count: cb }) => {
            *dst = AggState::SumI64 {
                sum: a + b,
                count: ca + cb,
            };
        }
        (AggState::SumF64 { sum: a, count: ca }, AggState::SumF64 { sum: b, count: cb }) => {
            *dst = AggState::SumF64 {
                sum: a + b,
                count: ca + cb,
            };
        }
        (AggState::Min(a), AggState::Min(b)) => {
            let best = if b < a { b } else { a };
            *dst = AggState::Min(best);
        }
        (AggState::Max(a), AggState::Max(b)) => {
            let best = if b > a { b } else { a };
            *dst = AggState::Max(best);
        }
        (AggState::CountDistinct(mut a), AggState::CountDistinct(b)) => {
            a.extend(b);
            *dst = AggState::CountDistinct(a);
        }
        (AggState::None, other) | (other, AggState::None) => {
            *dst = other;
        }
        (a, _b) => {
            *dst = a;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dap_core::LogicalType;
    use dap_storage::column::ColumnBuilder;

    fn input() -> RecordBatch {
        let schema = Schema::new(vec![
            Column::new("ou", LogicalType::Uid).required(),
            Column::new("period", LogicalType::Period).required(),
            Column::new("v", LogicalType::F64),
        ])
        .unwrap();
        let mut ou = ColumnBuilder::new(LogicalType::Uid);
        let mut per = ColumnBuilder::new(LogicalType::Period);
        let mut v = ColumnBuilder::new(LogicalType::F64);
        let rows = [
            ("a", "202601", Some(10.0)),
            ("b", "202601", Some(20.0)),
            ("a", "202601", Some(5.0)),
            ("a", "202602", Some(7.0)),
            ("b", "202602", None),
        ];
        for (o, p, x) in rows {
            ou.append_value(&Value::Str(o.into())).unwrap();
            per.append_value(&Value::Str(p.into())).unwrap();
            match x {
                Some(f) => v.append_value(&Value::F64(f)).unwrap(),
                None => v.append_value(&Value::Null).unwrap(),
            }
        }
        RecordBatch::new(schema, vec![ou.finish(), per.finish(), v.finish()]).unwrap()
    }

    #[test]
    fn sum_by_ou() {
        let batch = input();
        let mut agg = HashAggregator::new(
            batch.schema.clone(),
            vec!["ou".into()],
            vec![AggregateKind::Sum { column: "v".into() }],
        )
        .unwrap();
        let ctx = ExecContext::default();
        agg.consume_batch(&batch, &ctx).unwrap();
        let out = agg.finish(&ctx).unwrap();
        // Each row has (ou, sum_v)
        let mut by_ou = ahash::AHashMap::new();
        for r in 0..out.row_count() {
            let row = out.row_values(r).unwrap();
            by_ou.insert(row[0].clone(), row[1].clone());
        }
        assert_eq!(by_ou[&Value::Str("a".into())], Value::F64(22.0));
        assert_eq!(by_ou[&Value::Str("b".into())], Value::F64(20.0));
    }

    #[test]
    fn count_distinct_by_period() {
        let batch = input();
        let mut agg = HashAggregator::new(
            batch.schema.clone(),
            vec!["period".into()],
            vec![AggregateKind::CountDistinct { column: "ou".into() }],
        )
        .unwrap();
        let ctx = ExecContext::default();
        agg.consume_batch(&batch, &ctx).unwrap();
        let out = agg.finish(&ctx).unwrap();
        let mut by_period = ahash::AHashMap::new();
        for r in 0..out.row_count() {
            let row = out.row_values(r).unwrap();
            by_period.insert(row[0].clone(), row[1].clone());
        }
        assert_eq!(by_period[&Value::Str("202601".into())], Value::I64(2));
        assert_eq!(by_period[&Value::Str("202602".into())], Value::I64(2));
    }

    #[test]
    fn avg_handles_nulls() {
        let batch = input();
        let mut agg = HashAggregator::new(
            batch.schema.clone(),
            vec!["ou".into()],
            vec![AggregateKind::Avg { column: "v".into() }],
        )
        .unwrap();
        let ctx = ExecContext::default();
        agg.consume_batch(&batch, &ctx).unwrap();
        let out = agg.finish(&ctx).unwrap();
        let mut by_ou = ahash::AHashMap::new();
        for r in 0..out.row_count() {
            let row = out.row_values(r).unwrap();
            by_ou.insert(row[0].clone(), row[1].clone());
        }
        // ou=a: (10+5+7)/3 = 7.333..
        if let Value::F64(x) = by_ou[&Value::Str("a".into())] {
            assert!((x - 22.0 / 3.0).abs() < 1e-9);
        } else {
            panic!("expected F64");
        }
        // ou=b: (20)/1 = 20 (null dropped)
        assert_eq!(by_ou[&Value::Str("b".into())], Value::F64(20.0));
    }

    #[test]
    fn hash_aggregate_spills_and_recombines() {
        // Aggressively low spill threshold so the aggregator spills between batches and
        // still produces the correct totals after `finish` merges them back.
        let batch = input();
        let mut agg = HashAggregator::new(
            batch.schema.clone(),
            vec!["ou".into()],
            vec![AggregateKind::Sum { column: "v".into() }],
        )
        .unwrap();
        let ctx = ExecContext {
            memory: crate::budget::MemoryBudget {
                hard_limit: 1,
                spill_threshold: 1,
            },
            ..Default::default()
        };
        agg.consume_batch(&batch, &ctx).unwrap();
        // Consume a second slice to force more spills.
        agg.consume_batch(&batch, &ctx).unwrap();
        let out = agg.finish(&ctx).unwrap();
        let mut by_ou = ahash::AHashMap::new();
        for r in 0..out.row_count() {
            let row = out.row_values(r).unwrap();
            by_ou.insert(row[0].clone(), row[1].clone());
        }
        // Each input batch contributes ouA=22, ouB=20. Two consumptions => doubled.
        assert_eq!(by_ou[&Value::Str("a".into())], Value::F64(44.0));
        assert_eq!(by_ou[&Value::Str("b".into())], Value::F64(40.0));
    }

    #[test]
    fn min_max_across_groups() {
        let batch = input();
        let mut agg = HashAggregator::new(
            batch.schema.clone(),
            vec!["ou".into()],
            vec![
                AggregateKind::Min { column: "v".into() },
                AggregateKind::Max { column: "v".into() },
            ],
        )
        .unwrap();
        let ctx = ExecContext::default();
        agg.consume_batch(&batch, &ctx).unwrap();
        let out = agg.finish(&ctx).unwrap();
        let mut by_ou = ahash::AHashMap::new();
        for r in 0..out.row_count() {
            let row = out.row_values(r).unwrap();
            by_ou.insert(row[0].clone(), (row[1].clone(), row[2].clone()));
        }
        assert_eq!(
            by_ou[&Value::Str("a".into())],
            (Value::F64(5.0), Value::F64(10.0))
        );
        assert_eq!(
            by_ou[&Value::Str("b".into())],
            (Value::F64(20.0), Value::F64(20.0))
        );
    }
}
