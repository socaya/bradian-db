//! Zone-map / segment pruning (STO-007).
//!
//! Before the executor opens a segment, the planner checks the segment's per-column zone
//! map against the query predicate. If the zone map proves the predicate is false for
//! every row of the segment, the executor skips it entirely.
//!
//! The check is *conservative*: it must never cause a false negative. If any uncertainty
//! remains (e.g. the predicate touches a column without a zone map, or the zone map
//! includes nulls on both sides of a comparison), the segment is kept.
//!
//! The zone map stores `min` and `max` as the sort-order-preserving byte encoding the
//! writer applies (`signed_to_sortable` / `float_to_sortable` / lexicographic bytes for
//! strings — see `dap-storage/segment/writer.rs`). We convert the query literal into the
//! same byte form before the comparison.

use dap_core::{LogicalType, Schema, Value};
use dap_runtime::expr::{CmpOp, Expr, Predicate};
use dap_storage::{ColumnMeta, ManifestSegment, SegmentReader};

/// Decide whether a segment could contribute any row satisfying `predicate`.
pub fn segment_can_contribute(
    seg: &ManifestSegment,
    schema: &Schema,
    predicate: &Predicate,
) -> bool {
    // Open the footer-only to read column zone maps. If we cannot read the segment, be
    // conservative and keep it.
    let reader = match SegmentReader::open(&seg.path) {
        Ok(r) => r,
        Err(_) => return true,
    };
    let columns: Vec<&ColumnMeta> = reader.columns().iter().collect();
    predicate_can_be_true(predicate, schema, &columns)
}

fn predicate_can_be_true(p: &Predicate, schema: &Schema, cols: &[&ColumnMeta]) -> bool {
    let _ = schema; // reserved for typed comparisons once we add numeric range narrowing
    match p {
        Predicate::True => true,
        Predicate::False => false,
        Predicate::And(subs) => subs.iter().all(|s| predicate_can_be_true(s, schema, cols)),
        Predicate::Or(subs) => subs.iter().any(|s| predicate_can_be_true(s, schema, cols)),
        Predicate::Not(_) | Predicate::IsNull(_) | Predicate::IsNotNull(_) => true,
        Predicate::Compare { left, op, right } => compare_can_match(left, *op, right, schema, cols),
        Predicate::In { expr, values } => in_list_can_match(expr, values, schema, cols),
    }
}

fn compare_can_match(
    left: &Expr,
    op: CmpOp,
    right: &Expr,
    _schema: &Schema,
    cols: &[&ColumnMeta],
) -> bool {
    let (col_name, lit, flipped) = match (left, right) {
        (Expr::Col(c), Expr::Lit(v)) => (c.as_str(), v, false),
        (Expr::Lit(v), Expr::Col(c)) => (c.as_str(), v, true),
        _ => return true, // non-trivial expression; be conservative
    };
    let col = match cols.iter().find(|c| c.name == col_name) {
        Some(c) => *c,
        None => return true,
    };
    let (min, max) = match (&col.zone_map.min, &col.zone_map.max) {
        (Some(mi), Some(ma)) => (mi.clone(), ma.clone()),
        _ => return true,
    };
    let needle = match encode_sortable(lit, col.logical_type) {
        Some(b) => b,
        None => return true, // type mismatch; be conservative
    };
    // Flip comparison op if the literal was on the left.
    let op = if flipped { flip_cmp(op) } else { op };
    match op {
        CmpOp::Eq => needle >= min && needle <= max,
        CmpOp::NotEq => min != max || needle != min,
        CmpOp::Lt => needle > min,
        CmpOp::LtEq => needle >= min,
        CmpOp::Gt => needle < max,
        CmpOp::GtEq => needle <= max,
    }
}

fn in_list_can_match(
    expr: &Expr,
    values: &[Value],
    _schema: &Schema,
    cols: &[&ColumnMeta],
) -> bool {
    let col_name = match expr {
        Expr::Col(c) => c.as_str(),
        _ => return true,
    };
    let col = match cols.iter().find(|c| c.name == col_name) {
        Some(c) => *c,
        None => return true,
    };
    let (min, max) = match (&col.zone_map.min, &col.zone_map.max) {
        (Some(mi), Some(ma)) => (mi.clone(), ma.clone()),
        _ => return true,
    };
    values.iter().any(|v| {
        encode_sortable(v, col.logical_type)
            .map(|b| b >= min && b <= max)
            .unwrap_or(true)
    })
}

fn flip_cmp(op: CmpOp) -> CmpOp {
    match op {
        CmpOp::Lt => CmpOp::Gt,
        CmpOp::LtEq => CmpOp::GtEq,
        CmpOp::Gt => CmpOp::Lt,
        CmpOp::GtEq => CmpOp::LtEq,
        other => other,
    }
}

fn encode_sortable(v: &Value, ty: LogicalType) -> Option<Vec<u8>> {
    match (v, ty) {
        (Value::I64(n), LogicalType::I64 | LogicalType::Timestamp) => {
            let flipped = (*n as u64) ^ (1u64 << 63);
            Some(flipped.to_be_bytes().to_vec())
        }
        (Value::I64(n), LogicalType::I32) => {
            let n32 = *n as i32;
            Some(n32.to_be_bytes().to_vec())
        }
        (Value::F64(f), LogicalType::F64) => {
            let mut bits = f.to_bits();
            if (bits & (1u64 << 63)) != 0 {
                bits ^= u64::MAX;
            } else {
                bits ^= 1u64 << 63;
            }
            Some(bits.to_be_bytes().to_vec())
        }
        (Value::Decimal128 { value, scale: _ }, LogicalType::Decimal { .. }) => {
            let flipped = (*value as u128) ^ (1u128 << 127);
            Some(flipped.to_be_bytes().to_vec())
        }
        (Value::Str(s), LogicalType::Utf8 | LogicalType::Uid | LogicalType::Period) => {
            Some(s.as_bytes().to_vec())
        }
        (Value::Uid(u), LogicalType::Uid | LogicalType::Utf8) => {
            Some(u.as_ref().as_bytes().to_vec())
        }
        (Value::Period(p), LogicalType::Period | LogicalType::Utf8) => {
            Some(p.as_bytes().to_vec())
        }
        (Value::Bool(b), LogicalType::Bool) => Some(vec![if *b { 1 } else { 0 }]),
        (Value::Timestamp(t), LogicalType::Timestamp) => {
            let flipped = (*t as u64) ^ (1u64 << 63);
            Some(flipped.to_be_bytes().to_vec())
        }
        _ => None,
    }
}

/// Partition-level helper: return the subset of visible segments whose zone maps make it
/// possible for `predicate` to match. Callers that cannot prove anything simply iterate the
/// full list; this helper is pure optimisation.
pub fn prune_segments<'a>(
    segments: &'a [ManifestSegment],
    schema: &Schema,
    predicate: &Predicate,
) -> Vec<&'a ManifestSegment> {
    segments
        .iter()
        .filter(|s| segment_can_contribute(s, schema, predicate))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dap_core::{Column, LogicalType, Schema, Value};
    use dap_runtime::expr::{CmpOp, Expr, Predicate};
    use dap_storage::{Partition, PartitionOptions};
    use tempfile::tempdir;

    fn open_part(root: &std::path::Path) -> std::sync::Arc<Partition> {
        let schema = Schema::new(vec![
            Column::new("ou", LogicalType::Uid).required(),
            Column::new("period", LogicalType::Period).required(),
            Column::new("value", LogicalType::F64),
        ])
        .unwrap();
        Partition::open(
            root,
            PartitionOptions {
                dataset_id: "d".into(),
                partition_id: "p".into(),
                schema,
                primary_key_columns: vec!["ou".into(), "period".into()],
                flush_rows: 1024,
                flush_bytes: 1 << 24,
            },
        )
        .unwrap()
    }

    #[test]
    fn prunes_segment_outside_period_range() {
        let dir = tempdir().unwrap();
        let p = open_part(dir.path());
        // Segment 1: 202601 only.
        for ou in ["ouAAAAAAAAA", "ouBBBBBBBBB"] {
            p.upsert_row(
                vec![
                    Value::Str(ou.into()),
                    Value::Str("202601".into()),
                    Value::F64(1.0),
                ],
                1,
            )
            .unwrap();
        }
        p.flush().unwrap();
        // Segment 2: 202612 only.
        for ou in ["ouAAAAAAAAA", "ouBBBBBBBBB"] {
            p.upsert_row(
                vec![
                    Value::Str(ou.into()),
                    Value::Str("202612".into()),
                    Value::F64(2.0),
                ],
                1,
            )
            .unwrap();
        }
        p.flush().unwrap();

        let segs = p.visible_segments();
        assert_eq!(segs.len(), 2, "two segments flushed");

        // Predicate: period < "202606". Segment 1 entirely matches; segment 2 is entirely
        // out of range.
        let predicate = Predicate::Compare {
            left: Expr::Col("period".into()),
            op: CmpOp::Lt,
            right: Expr::Lit(Value::Str("202606".into())),
        };
        let kept = prune_segments(&segs, p.schema(), &predicate);
        assert_eq!(kept.len(), 1, "only the earlier segment survives");
    }

    #[test]
    fn keeps_segment_when_zone_map_proves_nothing() {
        let dir = tempdir().unwrap();
        let p = open_part(dir.path());
        p.upsert_row(
            vec![
                Value::Str("ouAAAAAAAAA".into()),
                Value::Str("202601".into()),
                Value::F64(1.0),
            ],
            1,
        )
        .unwrap();
        p.flush().unwrap();
        let segs = p.visible_segments();
        // Predicate over `value` with equality to 1.0 should keep the segment.
        let predicate = Predicate::Compare {
            left: Expr::Col("value".into()),
            op: CmpOp::Eq,
            right: Expr::Lit(Value::F64(1.0)),
        };
        assert_eq!(prune_segments(&segs, p.schema(), &predicate).len(), 1);
    }

    #[test]
    fn in_list_pruning() {
        let dir = tempdir().unwrap();
        let p = open_part(dir.path());
        p.upsert_row(
            vec![
                Value::Str("ouAAAAAAAAA".into()),
                Value::Str("202601".into()),
                Value::F64(1.0),
            ],
            1,
        )
        .unwrap();
        p.flush().unwrap();
        let segs = p.visible_segments();
        // IN with only values outside [ouAAAAAAAAA, ouAAAAAAAAA] → segment should be pruned.
        let predicate = Predicate::In {
            expr: Expr::Col("ou".into()),
            values: vec![
                Value::Str("ouZZZZZZZZZ".into()),
                Value::Str("ouYYYYYYYYY".into()),
            ],
        };
        assert_eq!(prune_segments(&segs, p.schema(), &predicate).len(), 0);

        // IN that contains the actual OU → kept.
        let predicate = Predicate::In {
            expr: Expr::Col("ou".into()),
            values: vec![
                Value::Str("ouAAAAAAAAA".into()),
                Value::Str("ouZZZZZZZZZ".into()),
            ],
        };
        assert_eq!(prune_segments(&segs, p.schema(), &predicate).len(), 1);
    }
}
