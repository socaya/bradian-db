//! ADR 0293 — commit-time column projection via `CommitReceipt`.
//!
//! Exercises `MultiTableTransaction::commit_with_receipt` on
//! a two-table shared domain. After a mixed commit
//! (inserts + replaces + delete across both tables) the
//! receipt's per-table upsert map projects into
//! `bradian_runtime::RecordBatch`es that round-trip exactly
//! to the stored post-image rows.
//!
//! Scope: upsert post-images only. Delete-side tombstones
//! for the columnar consumer are deferred to ADR 0294.

use std::num::NonZeroUsize;

use bradian_core::{Column as CoreColumn, LogicalType, Schema as CoreSchema, Value as CoreValue};
use bradian_relational::transaction_domain::MultiTableStore;
use bradian_relational::typed_table::Value;
use bradian_relational::{Catalog, Column, DataType, PrimaryKey, Table, TableSpec};
use bradian_runtime::batch::{RecordBatch, RecordBatchBuilder};

fn tables() -> (Table, Table) {
    let mut catalog = Catalog::new();
    let db = catalog.create_database("hr").unwrap();
    let schema = catalog.create_schema(db, "public").unwrap();
    let employees = catalog.create_table(schema, TableSpec {
        name: "employees".into(),
        columns: vec![
            Column { name: "id".into(),     data_type: DataType::Int64, nullable: false },
            Column { name: "salary".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "label".into(),  data_type: DataType::Text,  nullable: true  },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
        ..Default::default()
    }).unwrap();
    let projects = catalog.create_table(schema, TableSpec {
        name: "projects".into(),
        columns: vec![
            Column { name: "id".into(),   data_type: DataType::Int64, nullable: false },
            Column { name: "owner".into(),data_type: DataType::Int64, nullable: false },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
        ..Default::default()
    }).unwrap();
    (catalog.table(employees).unwrap().clone(),
     catalog.table(projects).unwrap().clone())
}

fn core_schema_for(table: &Table) -> CoreSchema {
    let cols = table.columns.iter().map(|c| {
        let logical = match c.data_type {
            DataType::Int64 => LogicalType::I64,
            DataType::Text  => LogicalType::Utf8,
            ref other => panic!("ADR 0293 slice covers only Int64/Text; got {other:?}"),
        };
        let mut cc = CoreColumn::new(&c.name, logical);
        if !c.nullable { cc = cc.required(); }
        cc
    }).collect();
    CoreSchema::new(cols).unwrap()
}

fn to_core_value(data_type: &DataType, v: &Value) -> CoreValue {
    match (data_type, v) {
        (_, Value::Null)                       => CoreValue::Null,
        (DataType::Int64, Value::Int64(x))     => CoreValue::I64(*x),
        (DataType::Text,  Value::String(s))    => CoreValue::Str(s.clone()),
        other => panic!("ADR 0293 slice covers Int64/Text only; got {other:?}"),
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
    let mut filled = 0usize;
    for row in rows {
        let mapped: Vec<CoreValue> = row.iter().zip(types.iter())
            .map(|(v, dt)| to_core_value(dt, v)).collect();
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

fn rows_from_batches(batches: &[RecordBatch]) -> Vec<Vec<CoreValue>> {
    let mut rows = Vec::new();
    for b in batches {
        for r in 0..b.row_count() { rows.push(b.row_values(r).unwrap()); }
    }
    rows
}

#[test]
fn commit_receipt_projects_upserts_into_record_batches() {
    let dir = tempfile::tempdir().unwrap();
    let (employees, projects) = tables();
    let db = MultiTableStore::open(dir.path()).unwrap();
    db.register_table(employees.clone()).unwrap();
    db.register_table(projects.clone()).unwrap();

    // Seed state (separate commit so the second commit mixes
    // insert + replace + delete across both tables).
    {
        let mut tx = db.begin_transaction().unwrap();
        tx.insert(employees.id, &[Value::Int64(1), Value::Int64(100), Value::String("alpha".into())]).unwrap();
        tx.insert(employees.id, &[Value::Int64(2), Value::Int64(200), Value::Null]).unwrap();
        tx.insert(projects.id,  &[Value::Int64(900), Value::Int64(1)]).unwrap();
        tx.commit().unwrap();
    }

    // Mixed second commit: replace employee 1, insert employee 3,
    // delete employee 2, insert project 901.
    let seed_lsn = db.last_lsn();
    let receipt = {
        let mut tx = db.begin_transaction().unwrap();
        tx.replace(employees.id, &[Value::Int64(1), Value::Int64(150), Value::String("alpha".into())]).unwrap();
        tx.insert (employees.id, &[Value::Int64(3), Value::Int64(300), Value::String("gamma".into())]).unwrap();
        tx.delete (employees.id, &[Value::Int64(2)]).unwrap();
        tx.insert (projects.id,  &[Value::Int64(901), Value::Int64(3)]).unwrap();
        tx.commit_with_receipt().unwrap()
    };

    assert_eq!(receipt.lsn, seed_lsn + 1,
        "receipt LSN should advance by exactly one commit");
    assert_eq!(receipt.upserts_by_table.len(), 2,
        "both tables should appear in the receipt");

    // Employees: receipt carries the replace + the insert, NOT the delete.
    let emp_upserts = receipt.upserts_by_table.get(&employees.id).unwrap();
    assert_eq!(emp_upserts.len(), 2,
        "employee upserts should exclude the delete (ADR 0293 scope)");
    let proj_upserts = receipt.upserts_by_table.get(&projects.id).unwrap();
    assert_eq!(proj_upserts.len(), 1);

    // Build RecordBatches from the receipt and assert the
    // materialised rows match the stored post-image rows.
    let emp_schema  = core_schema_for(&employees);
    let proj_schema = core_schema_for(&projects);
    let emp_batches = batches_from_rows(
        emp_upserts, &employees, &emp_schema, NonZeroUsize::new(16).unwrap());
    let proj_batches = batches_from_rows(
        proj_upserts, &projects, &proj_schema, NonZeroUsize::new(16).unwrap());

    // Mapping of the raw relational upsert rows via the same
    // adapter — this is the ground truth for the receipt side.
    let expected_emp: Vec<Vec<CoreValue>> = emp_upserts.iter().map(|row|
        row.iter().zip(employees.columns.iter())
           .map(|(v, c)| to_core_value(&c.data_type, v)).collect()).collect();
    let expected_proj: Vec<Vec<CoreValue>> = proj_upserts.iter().map(|row|
        row.iter().zip(projects.columns.iter())
           .map(|(v, c)| to_core_value(&c.data_type, v)).collect()).collect();

    assert_eq!(rows_from_batches(&emp_batches),  expected_emp,
        "employee batches do not round-trip the receipt rows");
    assert_eq!(rows_from_batches(&proj_batches), expected_proj,
        "project batches do not round-trip the receipt rows");

    // Verify durability: reopen and confirm the post-commit
    // state matches what the receipt said was upserted, and
    // the deleted row is gone.
    drop(db);
    let db = MultiTableStore::open(dir.path()).unwrap();
    let tx = db.begin_transaction().unwrap();
    assert_eq!(tx.get(employees.id, &[Value::Int64(1)]).unwrap(),
        Some(vec![Value::Int64(1), Value::Int64(150), Value::String("alpha".into())]));
    assert_eq!(tx.get(employees.id, &[Value::Int64(2)]).unwrap(), None,
        "employee 2 should have been deleted by the mixed commit");
    assert_eq!(tx.get(employees.id, &[Value::Int64(3)]).unwrap(),
        Some(vec![Value::Int64(3), Value::Int64(300), Value::String("gamma".into())]));
    assert_eq!(tx.get(projects.id,  &[Value::Int64(901)]).unwrap(),
        Some(vec![Value::Int64(901), Value::Int64(3)]));
}

#[test]
fn commit_receipt_is_empty_for_read_only_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let (employees, _projects) = tables();
    let db = MultiTableStore::open(dir.path()).unwrap();
    db.register_table(employees.clone()).unwrap();
    let base = db.last_lsn();
    let tx = db.begin_transaction().unwrap();
    let receipt = tx.commit_with_receipt().unwrap();
    assert_eq!(receipt.lsn, base,
        "read-only commit should not advance the LSN");
    assert!(receipt.upserts_by_table.is_empty(),
        "read-only commit should produce no upserts");
}

#[test]
fn legacy_commit_still_returns_only_the_lsn() {
    // Behaviour-preservation: `commit()` must remain usable
    // by existing callers and still return just the LSN.
    let dir = tempfile::tempdir().unwrap();
    let (employees, _projects) = tables();
    let db = MultiTableStore::open(dir.path()).unwrap();
    db.register_table(employees.clone()).unwrap();
    let mut tx = db.begin_transaction().unwrap();
    tx.insert(employees.id,
        &[Value::Int64(42), Value::Int64(0), Value::Null]).unwrap();
    let start = tx.start_lsn();
    let lsn = tx.commit().unwrap();
    assert_eq!(lsn, start + 1);
    assert_eq!(db.last_lsn(), lsn);
}
