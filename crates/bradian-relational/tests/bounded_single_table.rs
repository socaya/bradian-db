use bradian_relational::{Column,DataType,PrimaryKey,SchemaId,Table,TableId};
use bradian_relational::row_store::RowStore;
use bradian_relational::runtime_limits::{ResourceError,ScanControl,ScanLimits};
use bradian_relational::typed_table::{TableError,TableStore,Value};
use bradian_relational::sql::{SqlEngine,SqlError,SqlResultSet};

fn control(rows:usize,bytes:usize,work:usize)->ScanControl { ScanControl::new(ScanLimits { max_rows:rows,max_bytes:bytes,max_work:work }) }
fn table()->Table {
    Table { id:TableId(1),schema:SchemaId(1),name:"items".into(),columns:vec![
        Column { name:"id".into(),data_type:DataType::Int64,nullable:false },
        Column { name:"body".into(),data_type:DataType::Text,nullable:false }],
        primary_key:PrimaryKey { columns:vec!["id".into()] },unique_keys:vec![],foreign_keys:vec![],column_defaults:Default::default() }
}
fn row(id:i64,body:&str)->Vec<Value> { vec![Value::Int64(id),Value::String(body.into())] }
fn engine(root:&std::path::Path)->SqlEngine {
    let db=SqlEngine::open(root).unwrap();
    for sql in ["CREATE DATABASE test","CREATE SCHEMA test.public","CREATE TABLE test.public.items(id BIGINT NOT NULL,body TEXT NOT NULL,PRIMARY KEY(id))",
        "INSERT INTO test.public.items(id,body) VALUES(1,'one')","INSERT INTO test.public.items(id,body) VALUES(2,'two')"] { db.execute(sql).unwrap(); }
    db
}

#[test]
fn raw_snapshot_captures_one_lsn_with_exact_copy_budgets_and_excludes_only_metadata() {
    let dir=tempfile::tempdir().unwrap(); let store=RowStore::open(dir.path()).unwrap();
    store.put(b"meta",b"schema").unwrap(); store.put(b"a",b"v").unwrap(); store.put(b"b",b"w").unwrap();
    let lsn=store.last_lsn(); let bytes=2*(std::mem::size_of::<(Vec<u8>,Vec<u8>)>()+2);
    let (snapshot_lsn,rows)=store.snapshot_with_control(Some(b"meta"),&control(2,bytes,3)).unwrap();
    assert_eq!(snapshot_lsn,lsn); assert_eq!(rows,vec![(b"a".to_vec(),b"v".to_vec()),(b"b".to_vec(),b"w".to_vec())]);
    for (limits,resource) in [(control(1,bytes,3),"rows"),(control(2,bytes-1,3),"bytes"),(control(2,bytes,2),"work")] {
        assert!(matches!(store.snapshot_with_control(Some(b"meta"),&limits),Err(ResourceError::Limit { resource:actual,.. }) if actual==resource));
    }
    let cancelled=ScanControl::default(); cancelled.cancel();
    assert_eq!(store.snapshot_with_control(None,&cancelled),Err(ResourceError::Cancelled));
    assert_eq!(store.last_lsn(),lsn); assert_eq!(store.snapshot_with_lsn().1.len(),3);
}

#[test]
fn begin_rejects_oversized_decoded_snapshot_without_changing_native_compatibility() {
    let dir=tempfile::tempdir().unwrap();
    { let store=TableStore::open(dir.path(),table()).unwrap(); store.insert(&row(1,"x")).unwrap(); store.insert(&row(2,"y")).unwrap(); }
    let raw=RowStore::open(dir.path()).unwrap();
    let bytes=raw.scan().iter().filter(|(key,_)|key.starts_with(b"BRPK")).map(|(key,value)|key.len()+value.len()+std::mem::size_of::<(Vec<u8>,Vec<u8>)>()).sum();
    let lsn=raw.last_lsn(); drop(raw);
    let store=TableStore::open(dir.path(),table()).unwrap();
    assert!(matches!(store.begin_transaction_with_control(&control(2,bytes,3)),Err(TableError::Resource(ResourceError::Limit { resource:"bytes",.. }))));
    assert!(matches!(store.begin_transaction_with_control(&control(1,10000,3)),Err(TableError::Resource(ResourceError::Limit { resource:"rows",.. }))));
    let tx=store.begin_transaction().unwrap(); assert_eq!(tx.scan().len(),2); tx.rollback();
    assert_eq!(store.last_lsn(),lsn);
}

#[test]
fn controlled_scan_merge_matches_legacy_oracle_and_keeps_owned_snapshot_conflicts() {
    let dir=tempfile::tempdir().unwrap(); let store=TableStore::open(dir.path(),table()).unwrap();
    store.insert(&row(1,"one")).unwrap(); store.insert(&row(2,"two")).unwrap();
    let mut tx=store.begin_transaction_with_control(&control(2,10000,3)).unwrap();
    tx.replace(&row(1,"mine")).unwrap(); tx.delete(&[Value::Int64(2)]).unwrap(); tx.insert(&row(3,"three")).unwrap();
    assert_eq!(tx.scan_with_control(&control(2,10000,3)).unwrap(),tx.scan());
    assert!(matches!(tx.scan_with_control(&control(1,10000,3)),Err(TableError::Resource(ResourceError::Limit { resource:"rows",.. }))));
    assert!(matches!(tx.scan_with_control(&control(2,10000,2)),Err(TableError::Resource(ResourceError::Limit { resource:"work",.. }))));
    store.replace(&row(2,"outside")).unwrap();
    assert_eq!(tx.scan_with_control(&control(2,10000,3)).unwrap(),vec![row(1,"mine"),row(3,"three")]);
    assert!(matches!(tx.commit(),Err(TableError::Conflict)));
    assert_eq!(store.get(&[Value::Int64(1)]).unwrap(),Some(row(1,"one")));
}

#[test]
fn unexpected_namespaces_and_wrong_table_keys_are_not_hidden_by_controlled_scans() {
    for invalid_namespace in [true,false] {
        let dir=tempfile::tempdir().unwrap();
        { let store=TableStore::open(dir.path(),table()).unwrap(); store.insert(&row(1,"one")).unwrap(); }
        let raw=RowStore::open(dir.path()).unwrap();
        let (mut key,value)=raw.scan().into_iter().find(|(key,_)|key.starts_with(b"BRPK")).unwrap();
        if invalid_namespace { key=b"unknown-key".to_vec(); } else { key[5..13].copy_from_slice(&999_u64.to_le_bytes()); }
        raw.put(&key,&value).unwrap(); let lsn=raw.last_lsn(); drop(raw);
        let store=TableStore::open(dir.path(),table()).unwrap();
        assert!(matches!(store.scan_with_control(&control(10,10000,10)),Err(TableError::Corruption(_))));
        assert!(matches!(store.begin_transaction_with_control(&control(10,10000,10)),Err(TableError::Corruption(_))));
        assert_eq!(store.last_lsn(),lsn);
    }
}

#[test]
fn autocommit_select_enforces_scan_budgets_and_binding_precedes_scanning() {
    let dir=tempfile::tempdir().unwrap(); let db=engine(dir.path());
    let limits=control(1,10000,3);
    assert!(matches!(db.execute_with_control("SELECT id FROM test.public.items",&limits),Err(SqlError::Table(TableError::Resource(ResourceError::Limit { resource:"rows",.. })))));
    assert!(matches!(db.execute_with_control("SELECT id FROM test.public.items WHERE id=1",&limits).unwrap(),SqlResultSet::Rows { rows,.. } if rows==vec![vec![Value::Int64(1)]]));
    assert!(matches!(db.execute_with_control("SELECT missing FROM test.public.items",&control(0,0,0)),Err(SqlError::Resolution(_))));
    assert!(matches!(db.execute_with_control("SELECT id FROM test.public.items ORDER BY id",&control(0,0,0)),Err(SqlError::Unsupported(_))));
}

#[test]
fn snapshot_and_later_scan_failures_abort_without_publishing_staged_changes() {
    let dir=tempfile::tempdir().unwrap(); let db=engine(dir.path());
    db.with_sql_session_control("test.public.items",control(1,10000,100),|s| {
        assert!(s.execute("BEGIN").is_err()); assert!(!s.in_transaction()); assert!(s.execute("COMMIT").is_err()); Ok(())
    }).unwrap();
    db.with_sql_session_control("test.public.items",control(2,10000,100),|s| {
        s.execute("BEGIN")?; s.execute("UPDATE test.public.items SET body='changed' WHERE id=1")?;
        s.execute("INSERT INTO test.public.items(id,body) VALUES(3,'three')")?;
        assert!(s.execute("SELECT id FROM test.public.items").is_err()); assert!(!s.in_transaction());
        assert!(s.execute("COMMIT").is_err()); Ok(())
    }).unwrap();
    assert!(matches!(db.execute("SELECT body FROM test.public.items WHERE id=1").unwrap(),SqlResultSet::Rows { rows,.. } if rows==vec![vec![Value::String("one".into())]]));
    assert!(matches!(db.execute("SELECT id FROM test.public.items WHERE id=3").unwrap(),SqlResultSet::Rows { rows,.. } if rows.is_empty()));
}

#[test]
fn repeated_projection_cannot_amplify_results_past_bytes_or_cell_work_budget() {
    let dir=tempfile::tempdir().unwrap(); let db=engine(dir.path());
    db.execute(&format!("UPDATE test.public.items SET body='{}' WHERE id=1","x".repeat(1000))).unwrap();
    let columns=vec!["body";100].join(",");
    let sql=format!("SELECT {columns} FROM test.public.items WHERE id=1");
    assert!(matches!(db.execute_with_control(&sql,&control(2,8000,10000)),Err(SqlError::Resource(ResourceError::Limit { resource:"bytes",.. }))));
    assert!(matches!(db.execute_with_control(&sql,&control(2,1000000,99)),Err(SqlError::Resource(ResourceError::Limit { resource:"work",.. }))));
    db.with_sql_session_control("test.public.items",control(2,8000,10000),|s| {
        s.execute("BEGIN")?; s.execute("UPDATE test.public.items SET body='changed' WHERE id=2")?;
        assert!(s.execute(&sql).is_err()); assert!(!s.in_transaction()); Ok(())
    }).unwrap();
    assert!(matches!(db.execute("SELECT body FROM test.public.items WHERE id=2").unwrap(),SqlResultSet::Rows { rows,.. } if rows==vec![vec![Value::String("two".into())]]));
}

#[test]
fn cancellation_drops_pending_legacy_transaction_and_fresh_control_still_works() {
    let dir=tempfile::tempdir().unwrap(); let db=engine(dir.path()); let limits=ScanControl::default(); let cancel=limits.clone();
    db.with_sql_session_control("test.public.items",limits,|s| {
        s.execute("BEGIN")?; s.execute("UPDATE test.public.items SET body='changed' WHERE id=1")?;
        std::thread::spawn(move ||cancel.cancel()).join().unwrap();
        assert!(matches!(s.execute("SELECT id FROM test.public.items WHERE id=1"),Err(SqlError::Resource(ResourceError::Cancelled))));
        assert!(!s.in_transaction()); assert!(s.execute("COMMIT").is_err()); Ok(())
    }).unwrap();
    assert!(matches!(db.execute("SELECT body FROM test.public.items WHERE id=1").unwrap(),SqlResultSet::Rows { rows,.. } if rows==vec![vec![Value::String("one".into())]]));
    let cancelled=ScanControl::default(); cancelled.cancel();
    assert!(matches!(db.with_sql_session_control("test.public.items",cancelled,|_|Ok(())),Err(SqlError::Resource(ResourceError::Cancelled))));
}
