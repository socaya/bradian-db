//! Real file-backed typed table CRUD, primary key validation and schema pinning.

use std::sync::Arc;
use std::thread;

use bradian_relational::{
    Catalog, Column, DataType, PrimaryKey, Table, TableSpec, UniqueKey,
};
use bradian_relational::typed_table::{decode_row, encode_row, TableError, TableStore, Value};
use tempfile::tempdir;

fn schema() -> Table {
    let mut catalog = Catalog::new();
    let database = catalog.create_database("billing").unwrap();
    let scope = catalog.create_schema(database, "public").unwrap();
    let table_id = catalog.create_table(scope, TableSpec {
        name: "invoices".into(),
        columns: vec![
            Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "label".into(), data_type: DataType::Varchar { max_length: 80 }, nullable: false },
            Column { name: "amount".into(), data_type: DataType::Decimal { precision: 12, scale: 2 }, nullable: false },
            Column { name: "paid".into(), data_type: DataType::Boolean, nullable: false },
            Column { name: "memo".into(), data_type: DataType::Text, nullable: true },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
    }).unwrap();
    catalog.table(table_id).unwrap().clone()
}

fn invoice(id: i64, label: &str) -> Vec<Value> {
    vec![
        Value::Int64(id),
        Value::String(label.into()),
        Value::Decimal(12000),
        Value::Boolean(false),
        Value::Null,
    ]
}

#[test]
fn insert_read_update_delete_and_reopen_are_durable() {
    let dir = tempdir().unwrap();
    let meta = schema();
    {
        let table = TableStore::open(dir.path(), meta.clone()).unwrap();
        table.insert(&invoice(100, "January")).unwrap();
        table.insert(&invoice(200, "February")).unwrap();
        assert_eq!(table.get(&[Value::Int64(100)]).unwrap(), Some(invoice(100, "January")));
        let mut updated = invoice(100, "Updated");
        updated[3] = Value::Boolean(true);
        table.replace(&updated).unwrap();
        assert!(matches!(table.insert(&invoice(100, "duplicate")), Err(TableError::DuplicateKey)));
        table.delete(&[Value::Int64(200)]).unwrap();
        table.checkpoint().unwrap();
        assert_eq!(table.scan().unwrap(), vec![updated.clone()]);
    }
    let reopened = TableStore::open(dir.path(), meta).unwrap();
    let value = reopened.get(&[Value::Int64(100)]).unwrap().unwrap();
    assert_eq!(value[1], Value::String("Updated".into()));
    assert_eq!(value[3], Value::Boolean(true));
    assert_eq!(reopened.get(&[Value::Int64(200)]).unwrap(), None);
}

#[test]
fn primary_key_duplicate_rejected_after_restart_without_checkpoint() {
    let dir = tempdir().unwrap();
    {
        let t = TableStore::open(dir.path(), schema()).unwrap();
        t.insert(&invoice(-1, "first")).unwrap();
    }
    let t = TableStore::open(dir.path(), schema()).unwrap();
    assert!(matches!(t.insert(&invoice(-1, "again")), Err(TableError::DuplicateKey)));
    assert_eq!(t.get(&[Value::Int64(-1)]).unwrap(), Some(invoice(-1, "first")));
}

#[test]
fn schema_mismatch_and_reused_table_directory_are_rejected() {
    let dir = tempdir().unwrap();
    let old = schema();
    {
        let t = TableStore::open(dir.path(), old.clone()).unwrap();
        t.insert(&invoice(1, "exists")).unwrap();
    }
    let mut changed = old.clone();
    changed.columns[1].data_type = DataType::Text;
    assert!(matches!(TableStore::open(dir.path(), changed), Err(TableError::Schema(_))));
    let mut changed_id = old;
    changed_id.id.0 += 1;
    assert!(matches!(TableStore::open(dir.path(), changed_id), Err(TableError::Schema(_))));
    assert_eq!(TableStore::open(dir.path(), schema()).unwrap().scan().unwrap().len(), 1);
}

#[test]
fn wrong_types_nulls_decimal_bounds_and_width_are_rejected() {
    let dir = tempdir().unwrap();
    let t = TableStore::open(dir.path(), schema()).unwrap();
    let mut row = invoice(1, "valid");

    row[0] = Value::Null;
    assert!(matches!(t.insert(&row), Err(TableError::Value(_))));
    row[0] = Value::Int32(1);
    assert!(matches!(t.insert(&row), Err(TableError::Value(_))));
    row[0] = Value::Int64(1);
    row[1] = Value::String("x".repeat(81));
    assert!(matches!(t.insert(&row), Err(TableError::Value(_))));
    row[1] = Value::String("valid".into());
    row[2] = Value::Decimal(1_000_000_000_000);
    assert!(matches!(t.insert(&row), Err(TableError::Value(_))));
    row[2] = Value::Decimal(100);
    row[4] = Value::Null;
    assert!(t.insert(&row).is_ok());
    assert_eq!(t.scan().unwrap().len(), 1);
}

#[test]
fn secondary_constraints_are_not_silently_ignored() {
    let dir = tempdir().unwrap();
    let mut table = schema();
    table.unique_keys.push(UniqueKey {
        name: "unique_invoice_label".into(),
        columns: vec!["label".into()],
    });
    assert!(matches!(TableStore::open(dir.path(), table), Err(TableError::Schema(_))));
}

#[test]
fn typed_row_encoding_is_deterministic_and_detects_corruption() {
    let table = schema();
    let row = invoice(42, "écriture");
    let encoded = encode_row(&table, &row).unwrap();
    assert_eq!(&encoded[..4], b"BRRW");
    assert_eq!(decode_row(&table, &encoded).unwrap(), row);
    assert_eq!(encode_row(&table, &row).unwrap(), encoded);

    let mut trailing = encoded.clone();
    trailing.push(255);
    assert!(matches!(decode_row(&table, &trailing), Err(TableError::Corruption(_))));
    let mut bad_id = encoded.clone();
    bad_id[5] ^= 255;
    assert!(matches!(decode_row(&table, &bad_id), Err(TableError::Corruption(_))));
    let mut bad_version = encoded.clone();
    bad_version[4] = 255;
    assert!(matches!(decode_row(&table, &bad_version), Err(TableError::Corruption(_))));
    let cut = &encoded[..encoded.len()-2];
    assert!(matches!(decode_row(&table, cut), Err(TableError::Corruption(_))));
}

#[test]
fn typed_rows_cover_all_supported_scalar_types() {
    let mut table = schema();
    table.columns = vec![
        Column { name: "id".into(), data_type: DataType::Uuid, nullable: false },
        Column { name: "i32".into(), data_type: DataType::Int32, nullable: false },
        Column { name: "i64".into(), data_type: DataType::Int64, nullable: false },
        Column { name: "time".into(), data_type: DataType::TimestampTz, nullable: false },
        Column { name: "text".into(), data_type: DataType::Text, nullable: false },
        Column { name: "flag".into(), data_type: DataType::Boolean, nullable: true },
    ];
    let row = vec![
        Value::Uuid([0x12; 16]),
        Value::Int32(-77),
        Value::Int64(i64::MAX),
        Value::TimestampTz(1_700_000_000_000_000),
        Value::String("world 🌍".into()),
        Value::Null,
    ];
    let dir = tempdir().unwrap();
    let t = TableStore::open(dir.path(), table.clone()).unwrap();
    t.insert(&row).unwrap();
    assert_eq!(t.get(&[row[0].clone()]).unwrap(), Some(row.clone()));
    assert_eq!(decode_row(&table, &encode_row(&table, &row).unwrap()).unwrap(), row);
}

#[test]
fn concurrent_same_key_insert_yields_one_success_and_no_duplicates() {
    let dir = tempdir().unwrap();
    let table = Arc::new(TableStore::open(dir.path(), schema()).unwrap());
    let handles = (0..8).map(|i| {
        let table = Arc::clone(&table);
        thread::spawn(move || table.insert(&invoice(1, &format!("thread-{i}"))))
    }).collect::<Vec<_>>();
    let mut succeeded = 0;
    for thread in handles {
        match thread.join().unwrap() {
            Ok(_) => succeeded += 1,
            Err(TableError::DuplicateKey) => {}
            other => panic!("unexpected result: {other:?}"),
        }
    }
    assert_eq!(succeeded, 1);
    assert_eq!(table.scan().unwrap().len(), 1);
}

#[test]
fn composite_primary_keys_do_not_collide() {
    let mut table = schema();
    table.columns = vec![
        Column { name: "tenant".into(), data_type: DataType::Varchar { max_length: 80 }, nullable: false },
        Column { name: "code".into(), data_type: DataType::Varchar { max_length: 80 }, nullable: false },
    ];
    table.primary_key.columns = vec!["tenant".into(), "code".into()];
    let dir = tempdir().unwrap();
    let t = TableStore::open(dir.path(), table).unwrap();
    t.insert(&[Value::String("a".into()), Value::String("bc".into())]).unwrap();
    t.insert(&[Value::String("ab".into()), Value::String("c".into())]).unwrap();
    assert_eq!(t.scan().unwrap().len(), 2);
}

#[test]
fn replace_cannot_change_primary_key_and_delete_requires_existing_row() {
    let dir = tempdir().unwrap();
    let t = TableStore::open(dir.path(), schema()).unwrap();
    assert!(matches!(t.replace(&invoice(1, "missing")), Err(TableError::NotFound)));
    assert!(matches!(t.delete(&[Value::Int64(1)]), Err(TableError::NotFound)));
    t.insert(&invoice(1, "original")).unwrap();
    assert!(matches!(t.replace(&invoice(2, "different key")), Err(TableError::NotFound)));
    assert_eq!(t.get(&[Value::Int64(1)]).unwrap(), Some(invoice(1, "original")));
}


#[test]
fn point_reads_and_scans_reject_stored_row_corruption() {
    use bradian_relational::row_store::RowStore;

    let dir = tempdir().unwrap();
    let key;
    {
        let table = TableStore::open(dir.path(), schema()).unwrap();
        table.insert(&invoice(11, "original")).unwrap();
    }
    {
        let raw = RowStore::open(dir.path()).unwrap();
        key = raw.scan().into_iter().find(|(k, _)| k.starts_with(b"BRPK")).unwrap().0;
        raw.put(&key, b"not-a-typed-row").unwrap();
    }
    let reopened = TableStore::open(dir.path(), schema()).unwrap();
    assert!(matches!(reopened.get(&[Value::Int64(11)]), Err(TableError::Corruption(_))));
    assert!(matches!(reopened.scan(), Err(TableError::Corruption(_))));
}

#[test]
fn point_reads_verify_value_matches_physical_primary_key() {
    use bradian_relational::row_store::RowStore;

    let dir = tempdir().unwrap();
    let table_meta = schema();
    let key;
    {
        let table = TableStore::open(dir.path(), table_meta.clone()).unwrap();
        table.insert(&invoice(11, "original")).unwrap();
    }
    {
        let raw = RowStore::open(dir.path()).unwrap();
        key = raw.scan().into_iter().find(|(k, _)| k.starts_with(b"BRPK")).unwrap().0;
        raw.put(&key, &encode_row(&table_meta, &invoice(22, "wrong-key")).unwrap()).unwrap();
    }
    let reopened = TableStore::open(dir.path(), table_meta).unwrap();
    assert!(matches!(reopened.get(&[Value::Int64(11)]), Err(TableError::Corruption(_))));
    assert!(matches!(reopened.scan(), Err(TableError::Corruption(_))));
}
