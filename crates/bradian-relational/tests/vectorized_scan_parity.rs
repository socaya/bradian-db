//! ADR 0292 — vectorized SELECT path over RowStore rows.
//!
//! Opens a real `TableStore` backed by the WAL row store,
//! inserts rows, scans them out, maps them into
//! `bradian_runtime` batches, and asserts the batch
//! executor's filter + count matches the scalar oracle's
//! row-by-row filter + count. Also asserts a round-trip
//! from batch back to rows is one-for-one with the input.
//!
//! Scope is the shared-type subset: Int64, Text, Null.
//! Decimal / UUID / Timestamp / JSON cross-type mapping
//! is deferred to ADR 0293.
//!
//! The parity test is dev-only; `bradian-runtime` is not a
//! production dep of `bradian-relational` yet (see ADR 0292
//! Consequences).

use std::num::NonZeroUsize;

use bradian_core::{Column as CoreColumn, LogicalType, Schema as CoreSchema, Value as CoreValue};
use bradian_relational::typed_table::{TableStore, Value};
use bradian_relational::{Column, DataType, PrimaryKey, SchemaId, Table, TableId};
use bradian_runtime::aggregate::AggregateKind;
use bradian_runtime::batch::{RecordBatch, RecordBatchBuilder};
use bradian_runtime::budget::ExecContext;
use bradian_runtime::expr::{CmpOp, Expr, Predicate};
use bradian_runtime::parallel::parallel_filter_aggregate;

fn relational_table() -> Table {
    Table {
        id: TableId(1),
        schema: SchemaId(1),
        name: "items".into(),
        columns: vec![
            Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "score".into(), data_type: DataType::Int64, nullable: true },
            Column { name: "category".into(), data_type: DataType::Text, nullable: true },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
        foreign_keys: vec![],
        column_defaults: Default::default(),
        column_collations: Default::default(),
        policies: Default::default(),
        column_checks: Default::default(),
        column_default_exprs: Default::default(),
    }
}

fn core_schema() -> CoreSchema {
    CoreSchema::new(vec![
        CoreColumn::new("id", LogicalType::I64).required(),
        CoreColumn::new("score", LogicalType::I64),
        CoreColumn::new("category", LogicalType::Utf8),
    ])
    .unwrap()
}

fn to_core_value(data_type: &DataType, v: &Value) -> CoreValue {
    match (data_type, v) {
        (_, Value::Null) => CoreValue::Null,
        (DataType::Int64, Value::Int64(x)) => CoreValue::I64(*x),
        (DataType::Text, Value::String(s)) => CoreValue::Str(s.clone()),
        other => panic!(
            "ADR 0292 slice covers only (Int64, Text, Null); got {other:?}",
        ),
    }
}

fn batches_from_rows(
    rows: &[Vec<Value>],
    table: &Table,
    schema: &CoreSchema,
    batch_size: NonZeroUsize,
) -> Vec<RecordBatch> {
    let types: Vec<&DataType> = table.columns.iter().map(|c| &c.data_type).collect();
    let mut out = Vec::new();
    let mut builder = RecordBatchBuilder::new(schema.clone());
    let mut filled: usize = 0;
    for row in rows {
        let mapped: Vec<CoreValue> = row.iter().zip(types.iter())
            .map(|(v, dt)| to_core_value(dt, v))
            .collect();
        builder.append_row(&mapped).unwrap();
        filled += 1;
        if filled == batch_size.get() {
            out.push(builder.finish().unwrap());
            builder = RecordBatchBuilder::new(schema.clone());
            filled = 0;
        }
    }
    if filled > 0 { out.push(builder.finish().unwrap()); }
    out
}

/// Scalar row-by-row baseline for `score > threshold`.
fn scalar_count_score_gt(rows: &[Vec<Value>], threshold: i64) -> i64 {
    let mut n: i64 = 0;
    for row in rows {
        match &row[1] {
            Value::Int64(x) if *x > threshold => n += 1,
            _ => {}
        }
    }
    n
}

fn make_row(id: i64, score: Option<i64>, category: Option<&str>) -> Vec<Value> {
    vec![
        Value::Int64(id),
        match score { Some(x) => Value::Int64(x), None => Value::Null },
        match category { Some(c) => Value::String(c.into()), None => Value::Null },
    ]
}

#[test]
fn vectorized_row_scan_parity() {
    let dir = tempfile::tempdir().unwrap();
    let store = TableStore::open(dir.path(), relational_table()).unwrap();

    // Inputs mix non-null and null scores + categories so the
    // batch kernel exercises validity bitmaps as well as the
    // scalar comparison.
    let inputs: Vec<Vec<Value>> = vec![
        make_row(1,  Some(10),  Some("a")),
        make_row(2,  Some(55),  Some("b")),
        make_row(3,  Some(77),  None),
        make_row(4,  None,      Some("c")),
        make_row(5,  Some(100), Some("b")),
        make_row(6,  Some(42),  Some("a")),
        make_row(7,  Some(51),  Some("a")),
        make_row(8,  Some(49),  Some("b")),
        make_row(9,  Some(1000), None),
        make_row(10, None,      None),
    ];
    for row in &inputs { store.insert(row).unwrap(); }

    // Rows that actually came out of the RowStore WAL via the
    // typed-table scan path.
    let scanned: Vec<Vec<Value>> = store.scan().unwrap();
    assert_eq!(scanned.len(), inputs.len(),
        "WAL scan did not round-trip the inserted rows");

    let schema = core_schema();
    let table  = relational_table();
    let batches = batches_from_rows(
        &scanned,
        &table,
        &schema,
        NonZeroUsize::new(4).unwrap(),
    );
    // With batch_size=4 and 10 rows we expect 3 batches:
    // 4 + 4 + 2.
    assert_eq!(batches.iter().map(|b| b.row_count()).collect::<Vec<_>>(),
               vec![4, 4, 2],
               "unexpected batch shape");

    // Batch-executor filter: score > 50, COUNT(*).
    let predicate = Predicate::Compare {
        left:  Expr::Col("score".into()),
        op:    CmpOp::Gt,
        right: Expr::Lit(CoreValue::I64(50)),
    };
    let result = parallel_filter_aggregate(
        &batches,
        &predicate,
        /* group_by      */ vec![],
        /* aggregates    */ vec![AggregateKind::CountStar],
        /* input_schema  */ schema.clone(),
        /* ctx           */ &ExecContext::default(),
    ).unwrap();
    assert_eq!(result.row_count(), 1,
        "ungrouped aggregate should produce one result row");
    let batch_count = match result.row_values(0).unwrap().remove(0) {
        CoreValue::I64(n) => n,
        other => panic!("count aggregate returned non-i64: {other:?}"),
    };

    let scalar_count = scalar_count_score_gt(&scanned, 50);
    assert_eq!(batch_count, scalar_count,
        "batch-executor COUNT disagreed with scalar oracle");
    // Hand-verified: ids 2 (55), 3 (77), 5 (100), 7 (51), 9 (1000) = 5.
    assert_eq!(scalar_count, 5);

    // Round-trip: materialising each row from the batches
    // must reproduce the original mapped rows in order.
    let mut reconstructed: Vec<Vec<CoreValue>> = Vec::new();
    for batch in &batches {
        for r in 0..batch.row_count() {
            reconstructed.push(batch.row_values(r).unwrap());
        }
    }
    let expected: Vec<Vec<CoreValue>> = scanned.iter()
        .map(|row| row.iter().zip(table.columns.iter())
            .map(|(v, c)| to_core_value(&c.data_type, v))
            .collect())
        .collect();
    assert_eq!(reconstructed, expected,
        "batch row materialisation does not round-trip the scanned rows");
}
