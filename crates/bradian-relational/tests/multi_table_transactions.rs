//! G15: atomic cross-table transactions inside one native WAL domain.

use std::sync::Arc;
use std::thread;

use bradian_relational::{
    Catalog, Column, DataType, PrimaryKey, Table, TableId, TableSpec,
};
use bradian_relational::transaction_domain::{DomainError, MultiTableStore};
use bradian_relational::typed_table::{TableError, Value};
use tempfile::tempdir;

fn tables() -> (Table, Table) {
    let mut catalog = Catalog::new();
    let db = catalog.create_database("bank").unwrap();
    let schema = catalog.create_schema(db, "public").unwrap();
    let account = catalog.create_table(schema, TableSpec {
        name: "accounts".into(),
        columns: vec![
            Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "balance".into(), data_type: DataType::Int64, nullable: false },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
    }).unwrap();
    let movements = catalog.create_table(schema, TableSpec {
        name: "movements".into(),
        columns: vec![
            Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "account_id".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "amount".into(), data_type: DataType::Int64, nullable: false },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
    }).unwrap();
    (catalog.table(account).unwrap().clone(), catalog.table(movements).unwrap().clone())
}

fn balance(id: i64, amount: i64) -> Vec<Value> {
    vec![Value::Int64(id), Value::Int64(amount)]
}
fn movement(id: i64, account: i64, amount: i64) -> Vec<Value> {
    vec![Value::Int64(id), Value::Int64(account), Value::Int64(amount)]
}

#[test]
fn two_table_write_set_commits_in_one_wal_lsn_and_survives_reopen() {
    let dir = tempdir().unwrap();
    let (accounts, movements) = tables();
    let final_lsn;
    {
        let db = MultiTableStore::open(dir.path()).unwrap();
        db.register_table(accounts.clone()).unwrap();
        db.register_table(movements.clone()).unwrap();
        let mut tx = db.begin_transaction().unwrap();
        let start = tx.start_lsn();
        tx.insert(accounts.id, &balance(101, 850)).unwrap();
        tx.insert(movements.id, &movement(500, 101, -150)).unwrap();
        assert_eq!(tx.scan(accounts.id).unwrap(), vec![balance(101, 850)]);
        assert_eq!(tx.get(movements.id, &[Value::Int64(500)]).unwrap(),
            Some(movement(500, 101, -150)));
        assert_eq!(db.begin_transaction().unwrap().scan(accounts.id).unwrap().len(), 0);
        final_lsn = tx.commit().unwrap();
        assert_eq!(final_lsn, start + 1);
        assert_eq!(db.last_lsn(), final_lsn);
    }
    let db = MultiTableStore::open(dir.path()).unwrap();
    assert_eq!(db.last_lsn(), final_lsn);
    assert_eq!(db.table_count(), 2);
    let tx = db.begin_transaction().unwrap();
    assert_eq!(tx.get(accounts.id, &[Value::Int64(101)]).unwrap(), Some(balance(101, 850)));
    assert_eq!(tx.get(movements.id, &[Value::Int64(500)]).unwrap(), Some(movement(500, 101, -150)));
}

#[test]
fn multi_table_rollback_and_uncommitted_drop_change_no_rows() {
    let dir = tempdir().unwrap();
    let (accounts, movements) = tables();
    let db = MultiTableStore::open(dir.path()).unwrap();
    db.register_table(accounts.clone()).unwrap();
    db.register_table(movements.clone()).unwrap();
    let original = db.last_lsn();
    {
        let mut tx = db.begin_transaction().unwrap();
        tx.insert(accounts.id, &balance(1, 100)).unwrap();
        tx.insert(movements.id, &movement(1, 1, 100)).unwrap();
        tx.rollback();
    }
    {
        let mut tx = db.begin_transaction().unwrap();
        tx.insert(accounts.id, &balance(2, 200)).unwrap();
        tx.insert(movements.id, &movement(2, 2, 200)).unwrap();
    }
    assert_eq!(db.last_lsn(), original);
    let tx = db.begin_transaction().unwrap();
    assert!(tx.scan(accounts.id).unwrap().is_empty());
    assert!(tx.scan(movements.id).unwrap().is_empty());
}

#[test]
fn cross_table_replace_and_delete_persist_through_checkpoint() {
    let dir = tempdir().unwrap();
    let (accounts, movements) = tables();
    {
        let db = MultiTableStore::open(dir.path()).unwrap();
        db.register_table(accounts.clone()).unwrap();
        db.register_table(movements.clone()).unwrap();
        let mut first = db.begin_transaction().unwrap();
        first.insert(accounts.id, &balance(1, 100)).unwrap();
        first.insert(movements.id, &movement(1, 1, 20)).unwrap();
        first.commit().unwrap();
        let mut second = db.begin_transaction().unwrap();
        second.replace(accounts.id, &balance(1, 80)).unwrap();
        second.delete(movements.id, &[Value::Int64(1)]).unwrap();
        second.insert(movements.id, &movement(2, 1, -20)).unwrap();
        second.commit().unwrap();
        db.checkpoint().unwrap();
    }
    let db = MultiTableStore::open(dir.path()).unwrap();
    let tx = db.begin_transaction().unwrap();
    assert_eq!(tx.scan(accounts.id).unwrap(), vec![balance(1, 80)]);
    assert_eq!(tx.scan(movements.id).unwrap(), vec![movement(2, 1, -20)]);
}

#[test]
fn stale_global_snapshot_conflicts_across_tables() {
    let dir = tempdir().unwrap();
    let (accounts, movements) = tables();
    let db = MultiTableStore::open(dir.path()).unwrap();
    db.register_table(accounts.clone()).unwrap();
    db.register_table(movements.clone()).unwrap();
    let mut tx1 = db.begin_transaction().unwrap();
    let mut tx2 = db.begin_transaction().unwrap();
    tx1.insert(accounts.id, &balance(1, 10)).unwrap();
    tx2.insert(movements.id, &movement(1, 1, 10)).unwrap();
    tx1.commit().unwrap();
    assert!(matches!(tx2.commit(), Err(DomainError::Conflict)));
    let verify = db.begin_transaction().unwrap();
    assert_eq!(verify.scan(accounts.id).unwrap().len(), 1);
    assert!(verify.scan(movements.id).unwrap().is_empty());
}

#[test]
fn registration_changes_are_visible_only_after_new_transaction() {
    let dir = tempdir().unwrap();
    let (accounts, movements) = tables();
    let db = MultiTableStore::open(dir.path()).unwrap();
    db.register_table(accounts.clone()).unwrap();
    let mut old = db.begin_transaction().unwrap();
    old.insert(accounts.id, &balance(1, 11)).unwrap();
    db.register_table(movements.clone()).unwrap();
    assert!(matches!(
        old.insert(movements.id, &movement(1, 1, 11)),
        Err(DomainError::UnknownTable(_))
    ));
    assert!(matches!(old.commit(), Err(DomainError::Conflict)));
    let mut fresh = db.begin_transaction().unwrap();
    fresh.insert(movements.id, &movement(1, 1, 11)).unwrap();
    fresh.commit().unwrap();
}

#[test]
fn incompatible_table_id_reuse_is_rejected_even_after_restart() {
    let dir = tempdir().unwrap();
    let (accounts, _) = tables();
    {
        let db = MultiTableStore::open(dir.path()).unwrap();
        db.register_table(accounts.clone()).unwrap();
        assert_eq!(db.register_table(accounts.clone()).unwrap(), db.last_lsn());
    }
    let db = MultiTableStore::open(dir.path()).unwrap();
    let mut mismatch = accounts.clone();
    mismatch.columns[1].name = "new_balance".into();
    assert!(matches!(db.register_table(mismatch), Err(DomainError::SchemaMismatch(_))));
    assert_eq!(db.table_count(), 1);
}

#[test]
fn unregistered_table_and_malformed_row_are_not_silently_accepted() {
    let dir = tempdir().unwrap();
    let (accounts, movements) = tables();
    let db = MultiTableStore::open(dir.path()).unwrap();
    db.register_table(accounts.clone()).unwrap();
    let mut tx = db.begin_transaction().unwrap();
    assert!(matches!(tx.insert(movements.id, &movement(1, 1, 2)), Err(DomainError::UnknownTable(_))));
    assert!(matches!(
        tx.insert(accounts.id, &[Value::Int64(1), Value::Null]),
        Err(DomainError::Table(TableError::Value(_)))
    ));
    assert_eq!(db.last_lsn(), 1);
}

#[test]
fn too_many_distinct_cross_table_operations_fail_atomically() {
    let dir = tempdir().unwrap();
    let (accounts, movements) = tables();
    let db = MultiTableStore::open(dir.path()).unwrap();
    db.register_table(accounts.clone()).unwrap();
    db.register_table(movements.clone()).unwrap();
    let original = db.last_lsn();
    let mut tx = db.begin_transaction().unwrap();
    for i in 0..200 {
        tx.insert(accounts.id, &balance(i, i)).unwrap();
    }
    for i in 0..57 {
        tx.insert(movements.id, &movement(i, i, 10)).unwrap();
    }
    assert!(matches!(tx.commit(), Err(DomainError::Storage(_))));
    assert_eq!(db.last_lsn(), original);
    let snapshot = db.begin_transaction().unwrap();
    assert!(snapshot.scan(accounts.id).unwrap().is_empty());
    assert!(snapshot.scan(movements.id).unwrap().is_empty());
}

#[test]
fn multiple_threads_cannot_partially_publish_cross_table_changes() {
    let dir = tempdir().unwrap();
    let (accounts, movements) = tables();
    let db = Arc::new(MultiTableStore::open(dir.path()).unwrap());
    db.register_table(accounts.clone()).unwrap();
    db.register_table(movements.clone()).unwrap();
    let jobs = (0..8).map(|i| {
        let db = Arc::clone(&db);
        let a = accounts.id;
        let b = movements.id;
        thread::spawn(move || {
            let mut tx = db.begin_transaction().unwrap();
            tx.insert(a, &balance(i, 100)).unwrap();
            tx.insert(b, &movement(i, i, -100)).unwrap();
            tx.commit()
        })
    }).collect::<Vec<_>>();
    let mut success = 0;
    for job in jobs {
        match job.join().unwrap() {
            Ok(_) => success += 1,
            Err(DomainError::Conflict) => {}
            other => panic!("unexpected commit: {other:?}"),
        }
    }
    let tx = db.begin_transaction().unwrap();
    assert_eq!(tx.scan(accounts.id).unwrap().len(), success);
    assert_eq!(tx.scan(movements.id).unwrap().len(), success);
}

#[test]
fn duplicate_key_and_read_only_snapshot_have_expected_semantics() {
    let dir = tempdir().unwrap();
    let (accounts, _) = tables();
    let db = MultiTableStore::open(dir.path()).unwrap();
    db.register_table(accounts.clone()).unwrap();
    let mut tx = db.begin_transaction().unwrap();
    tx.insert(accounts.id, &balance(3, 50)).unwrap();
    assert!(matches!(
        tx.insert(accounts.id, &balance(3, 80)),
        Err(DomainError::Table(TableError::DuplicateKey))
    ));
    let committed = tx.commit().unwrap();
    let read = db.begin_transaction().unwrap();
    assert_eq!(read.start_lsn(), committed);
    assert_eq!(read.commit().unwrap(), committed);
    let verify = db.begin_transaction().unwrap();
    assert_eq!(verify.get(accounts.id, &[Value::Int64(3)]).unwrap(), Some(balance(3, 50)));
}
