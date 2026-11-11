//! End-to-end runtime check: ingest rows into a Partition, flush to a `.derc` segment, scan,
//! filter, aggregate, and verify the result matches the scalar oracle.

use dap_core::{Column, LogicalType, Schema, Value};
use dap_runtime::aggregate::{AggregateKind, HashAggregator};
use dap_runtime::budget::ExecContext;
use dap_runtime::expr::{CmpOp, Expr, Predicate};
use dap_runtime::oracle::aggregate_oracle;
use dap_runtime::scan::{scan_partition, ScanOptions};
use dap_storage::{Partition, PartitionOptions};
use tempfile::tempdir;

fn schema() -> Schema {
    Schema::new(vec![
        Column::new("ou", LogicalType::Uid).required(),
        Column::new("period", LogicalType::Period).required(),
        Column::new("de", LogicalType::Uid).required(),
        Column::new("value", LogicalType::F64),
    ])
    .unwrap()
}

fn seed_rows(p: &std::sync::Arc<Partition>) {
    let rows: &[(&str, &str, &str, Option<f64>)] = &[
        ("ouAAAAAAAAA", "202601", "deAAAAAAAAA", Some(10.0)),
        ("ouAAAAAAAAA", "202601", "deBBBBBBBBB", Some(5.0)),
        ("ouBBBBBBBBB", "202601", "deAAAAAAAAA", Some(20.0)),
        ("ouBBBBBBBBB", "202601", "deBBBBBBBBB", None),
        ("ouAAAAAAAAA", "202602", "deAAAAAAAAA", Some(12.5)),
        ("ouAAAAAAAAA", "202602", "deBBBBBBBBB", Some(7.5)),
        ("ouBBBBBBBBB", "202602", "deAAAAAAAAA", Some(25.0)),
    ];
    for (ou, per, de, v) in rows {
        let value = match v {
            Some(f) => Value::F64(*f),
            None => Value::Null,
        };
        p.upsert_row(
            vec![
                Value::Str((*ou).into()),
                Value::Str((*per).into()),
                Value::Str((*de).into()),
                value,
            ],
            1,
        )
        .unwrap();
    }
}

#[test]
fn scan_filter_aggregate_matches_oracle() {
    let dir = tempdir().unwrap();
    let p = Partition::open(
        dir.path(),
        PartitionOptions {
            dataset_id: "agg1".into(),
            partition_id: "p1".into(),
            schema: schema(),
            primary_key_columns: vec!["ou".into(), "period".into(), "de".into()],
            flush_rows: 1024,
            flush_bytes: 1 << 24,
        },
    )
    .unwrap();

    seed_rows(&p);
    let _seg = p.flush().unwrap().expect("expected a flushed segment");

    let ctx = ExecContext::default();
    let batches = scan_partition(&p, &ScanOptions::default(), &ctx).unwrap();
    assert!(!batches.is_empty());
    let total_rows: usize = batches.iter().map(|b| b.row_count()).sum();
    assert_eq!(total_rows, 7);

    let pred = Predicate::Compare {
        left: Expr::Col("period".into()),
        op: CmpOp::Eq,
        right: Expr::Lit(Value::Str("202601".into())),
    };

    // Vectorized pipeline.
    let mut agg = HashAggregator::new(
        schema(),
        vec!["ou".into()],
        vec![AggregateKind::Sum { column: "value".into() }],
    )
    .unwrap();
    for batch in &batches {
        let selection = pred.eval_batch(batch).unwrap();
        let mut builder = dap_runtime::batch::RecordBatchBuilder::new(batch.schema.clone());
        for &row in &selection {
            builder.append_row(&batch.row_values(row as usize).unwrap()).unwrap();
        }
        let filtered = builder.finish().unwrap();
        agg.consume_batch(&filtered, &ctx).unwrap();
    }
    let vec_out = agg.finish(&ctx).unwrap();

    // Oracle.
    let oracle_out = aggregate_oracle(
        &batches,
        &pred,
        &["ou".into()],
        &[AggregateKind::Sum { column: "value".into() }],
    )
    .unwrap();

    // Compare — both sides keyed by group.
    let mut vec_map = std::collections::BTreeMap::new();
    for r in 0..vec_out.row_count() {
        let row = vec_out.row_values(r).unwrap();
        vec_map.insert(row[0].clone(), row[1].clone());
    }
    let mut oracle_map = std::collections::BTreeMap::new();
    for (k, vs) in oracle_out {
        oracle_map.insert(k[0].clone(), vs[0].clone());
    }
    assert_eq!(vec_map, oracle_map);

    // Spot check: ouA sum for period 202601 = 10 + 5 = 15, ouB = 20 (null dropped).
    assert_eq!(
        vec_map[&Value::Str("ouAAAAAAAAA".into())],
        Value::F64(15.0)
    );
    assert_eq!(
        vec_map[&Value::Str("ouBBBBBBBBB".into())],
        Value::F64(20.0)
    );
}

#[test]
fn delta_rows_visible_to_scan_before_flush() {
    let dir = tempdir().unwrap();
    let p = Partition::open(
        dir.path(),
        PartitionOptions {
            dataset_id: "agg1".into(),
            partition_id: "p1".into(),
            schema: schema(),
            primary_key_columns: vec!["ou".into(), "period".into(), "de".into()],
            flush_rows: 1024,
            flush_bytes: 1 << 24,
        },
    )
    .unwrap();

    // Flush an initial batch.
    seed_rows(&p);
    p.flush().unwrap();

    // Upsert new rows that stay only in the delta.
    p.upsert_row(
        vec![
            Value::Str("ouCCCCCCCCC".into()),
            Value::Str("202601".into()),
            Value::Str("deAAAAAAAAA".into()),
            Value::F64(999.0),
        ],
        1,
    )
    .unwrap();

    // Scan must now merge the delta on top of the flushed segments.
    let ctx = ExecContext::default();
    let batches = scan_partition(&p, &ScanOptions::default(), &ctx).unwrap();
    let total: usize = batches.iter().map(|b| b.row_count()).sum();
    assert_eq!(total, 8, "delta row visible to scan without flush");
}

#[test]
fn delta_tombstone_removes_segment_row_from_scan() {
    let dir = tempdir().unwrap();
    let p = Partition::open(
        dir.path(),
        PartitionOptions {
            dataset_id: "agg1".into(),
            partition_id: "p1".into(),
            schema: schema(),
            primary_key_columns: vec!["ou".into(), "period".into(), "de".into()],
            flush_rows: 1024,
            flush_bytes: 1 << 24,
        },
    )
    .unwrap();

    seed_rows(&p);
    p.flush().unwrap();

    // Delete one key via the delta (no flush).
    p.delete_row(
        vec![
            Value::Str("ouAAAAAAAAA".into()),
            Value::Str("202601".into()),
            Value::Str("deAAAAAAAAA".into()),
        ],
        2,
    )
    .unwrap();

    let ctx = ExecContext::default();
    let batches = scan_partition(&p, &ScanOptions::default(), &ctx).unwrap();
    let total: usize = batches.iter().map(|b| b.row_count()).sum();
    assert_eq!(total, 6, "tombstoned row hidden from scan");
}
