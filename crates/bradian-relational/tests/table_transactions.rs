//! G13: durable and recoverable single-table optimistic transactions.

use std::sync::Arc;
use std::thread;

use bradian_relational::{Catalog, Column, DataType, PrimaryKey, Table, TableSpec};
use bradian_relational::typed_table::{TableError, TableStore, Value};
use tempfile::tempdir;

fn schema() -> Table {
    let mut catalog = Catalog::new();
    let db = catalog.create_database("accounts").unwrap();
    let s = catalog.create_schema(db, "public").unwrap();
    let id = catalog.create_table(s, TableSpec {
        name: "balances".into(),
        columns: vec![
            Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "balance".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "note".into(), data_type: DataType::Text, nullable: true },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
    }).unwrap();
    catalog.table(id).unwrap().clone()
}

fn row(id: i64, amount: i64) -> Vec<Value> {
    vec![Value::Int64(id), Value::Int64(amount), Value::Null]
}

#[test]
fn multi_row_commit_survives_restart_with_single_wal_lsn() {
    let dir = tempdir().unwrap();
    let lsn;
    {
        let store = TableStore::open(dir.path(), schema()).unwrap();
        let mut tx = store.begin_transaction().unwrap();
        let initial = tx.start_lsn();
        tx.insert(&row(1, 400)).unwrap();
        tx.insert(&row(2, 600)).unwrap();
        assert_eq!(store.get(&[Value::Int64(1)]).unwrap(), None);
        assert_eq!(tx.get(&[Value::Int64(1)]).unwrap(), Some(row(1, 400)));
        lsn = tx.commit().unwrap();
        assert_eq!(lsn, initial + 1);
        assert_eq!(store.last_lsn(), lsn);
    }
    let reopened = TableStore::open(dir.path(), schema()).unwrap();
    assert_eq!(reopened.last_lsn(), lsn);
    assert_eq!(reopened.get(&[Value::Int64(1)]).unwrap(), Some(row(1, 400)));
    assert_eq!(reopened.get(&[Value::Int64(2)]).unwrap(), Some(row(2, 600)));
    reopened.checkpoint().unwrap();
    drop(reopened);
    assert_eq!(TableStore::open(dir.path(), schema()).unwrap().scan().unwrap().len(), 2);
}

#[test]
fn rollback_and_drop_discard_staged_changes_without_wal() {
    let dir = tempdir().unwrap();
    let store = TableStore::open(dir.path(), schema()).unwrap();
    let before = store.last_lsn();
    {
        let mut tx = store.begin_transaction().unwrap();
        tx.insert(&row(7, 123)).unwrap();
        assert_eq!(tx.scan(), vec![row(7, 123)]);
        tx.rollback();
    }
    {
        let mut tx = store.begin_transaction().unwrap();
        tx.insert(&row(8, 300)).unwrap();
    }
    assert!(store.scan().unwrap().is_empty());
    assert_eq!(store.last_lsn(), before);
    drop(store);
    assert!(TableStore::open(dir.path(), schema()).unwrap().scan().unwrap().is_empty());
}

#[test]
fn staged_transfer_is_atomic_across_two_keys() {
    let dir = tempdir().unwrap();
    let store = TableStore::open(dir.path(), schema()).unwrap();
    store.insert(&row(1, 1000)).unwrap();
    store.insert(&row(2, 500)).unwrap();
    let mut tx = store.begin_transaction().unwrap();
    tx.replace(&row(1, 800)).unwrap();
    tx.replace(&row(2, 700)).unwrap();
    assert_eq!(tx.get(&[Value::Int64(1)]).unwrap(), Some(row(1, 800)));
    assert_eq!(tx.get(&[Value::Int64(2)]).unwrap(), Some(row(2, 700)));
    assert_eq!(store.get(&[Value::Int64(1)]).unwrap(), Some(row(1, 1000)));
    tx.commit().unwrap();
    assert_eq!(store.get(&[Value::Int64(1)]).unwrap(), Some(row(1, 800)));
    assert_eq!(store.get(&[Value::Int64(2)]).unwrap(), Some(row(2, 700)));
}

#[test]
fn stale_concurrent_transaction_cannot_overwrite_committed_changes() {
    let dir = tempdir().unwrap();
    let store = TableStore::open(dir.path(), schema()).unwrap();
    store.insert(&row(1, 500)).unwrap();
    let mut a = store.begin_transaction().unwrap();
    let mut b = store.begin_transaction().unwrap();
    a.replace(&row(1, 450)).unwrap();
    b.replace(&row(1, 400)).unwrap();
    a.commit().unwrap();
    assert!(matches!(b.commit(), Err(TableError::Conflict)));
    assert_eq!(store.get(&[Value::Int64(1)]).unwrap(), Some(row(1, 450)));
}

#[test]
fn concurrent_write_to_unrelated_key_still_conflicts_conservatively() {
    let dir = tempdir().unwrap();
    let store = TableStore::open(dir.path(), schema()).unwrap();
    let mut tx = store.begin_transaction().unwrap();
    tx.insert(&row(1, 1)).unwrap();
    store.insert(&row(2, 2)).unwrap();
    assert!(matches!(tx.commit(), Err(TableError::Conflict)));
    assert_eq!(store.scan().unwrap(), vec![row(2, 2)]);
}

#[test]
fn snapshot_reads_do_not_change_when_other_writers_commit() {
    let dir = tempdir().unwrap();
    let store = TableStore::open(dir.path(), schema()).unwrap();
    store.insert(&row(3, 100)).unwrap();
    let tx = store.begin_transaction().unwrap();
    store.replace(&row(3, 200)).unwrap();
    store.insert(&row(4, 400)).unwrap();
    assert_eq!(tx.get(&[Value::Int64(3)]).unwrap(), Some(row(3, 100)));
    assert_eq!(tx.get(&[Value::Int64(4)]).unwrap(), None);
    assert_eq!(tx.scan(), vec![row(3, 100)]);
    let snapshot_lsn = tx.start_lsn();
    assert_eq!(tx.commit().unwrap(), snapshot_lsn);
}

#[test]
fn staged_delete_and_insert_same_key_resolves_to_final_value() {
    let dir = tempdir().unwrap();
    let store = TableStore::open(dir.path(), schema()).unwrap();
    store.insert(&row(3, 400)).unwrap();
    let mut tx = store.begin_transaction().unwrap();
    tx.delete(&[Value::Int64(3)]).unwrap();
    assert_eq!(tx.get(&[Value::Int64(3)]).unwrap(), None);
    tx.insert(&row(3, 450)).unwrap();
    tx.commit().unwrap();
    assert_eq!(store.get(&[Value::Int64(3)]).unwrap(), Some(row(3, 450)));
}

#[test]
fn invalid_operation_does_not_partially_commit_other_staged_rows() {
    let dir = tempdir().unwrap();
    let store = TableStore::open(dir.path(), schema()).unwrap();
    let initial = store.last_lsn();
    let mut tx = store.begin_transaction().unwrap();
    tx.insert(&row(1, 100)).unwrap();
    let invalid = vec![Value::Int64(2), Value::Null, Value::Null];
    assert!(matches!(tx.insert(&invalid), Err(TableError::Value(_))));
    tx.rollback();
    assert!(store.scan().unwrap().is_empty());
    assert_eq!(store.last_lsn(), initial);
}

#[test]
fn too_many_staged_operations_fail_as_one_batch_without_writes() {
    let dir = tempdir().unwrap();
    let store = TableStore::open(dir.path(), schema()).unwrap();
    let initial = store.last_lsn();
    let mut tx = store.begin_transaction().unwrap();
    for i in 0..257 {
        tx.insert(&row(i, i)).unwrap();
    }
    assert!(matches!(tx.commit(), Err(TableError::Storage(_))));
    assert!(store.scan().unwrap().is_empty());
    assert_eq!(store.last_lsn(), initial);
}

#[test]
fn transactions_are_safe_with_multiple_threads_on_one_store() {
    let dir = tempdir().unwrap();
    let store = Arc::new(TableStore::open(dir.path(), schema()).unwrap());
    let threads = (0..8).map(|i| {
        let store = Arc::clone(&store);
        thread::spawn(move || {
            let mut tx = store.begin_transaction().unwrap();
            tx.insert(&row(i, 1)).unwrap();
            tx.commit()
        })
    }).collect::<Vec<_>>();
    let mut successful = 0;
    for t in threads {
        match t.join().unwrap() {
            Ok(_) => successful += 1,
            Err(TableError::Conflict) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }
    assert!(successful >= 1);
    assert_eq!(store.scan().unwrap().len(), successful);
}

#[test]
fn committed_tombstones_survive_checkpoint_and_restart() {
    let dir = tempdir().unwrap();
    {
        let store = TableStore::open(dir.path(), schema()).unwrap();
        store.insert(&row(1, 1)).unwrap();
        store.insert(&row(2, 2)).unwrap();
        let mut tx = store.begin_transaction().unwrap();
        tx.delete(&[Value::Int64(1)]).unwrap();
        tx.insert(&row(3, 3)).unwrap();
        tx.commit().unwrap();
        store.checkpoint().unwrap();
    }
    let reopened = TableStore::open(dir.path(), schema()).unwrap();
    assert_eq!(reopened.get(&[Value::Int64(1)]).unwrap(), None);
    assert_eq!(reopened.scan().unwrap(), vec![row(2, 2), row(3, 3)]);
}
