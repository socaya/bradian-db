#[path = "support/epoch_fixture.rs"]
mod fixture;
use fixture::{migrated, modify, query};
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::sql::SqlResultSet;
use bradian_relational::typed_table::Value;

#[test]
fn cross_table_savepoints_commit_only_selected_work_and_preserve_history() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    let baseline = db.last_lsn();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE bank.public.accounts SET balance=110 WHERE id=1")?;
        s.execute("SAVEPOINT Work")?;
        s.execute("UPDATE bank.public.audit SET balance=999 WHERE id=9")?;
        s.execute("INSERT INTO bank.public.messages(id,body) VALUES(1,'discard')")?;
        s.execute("SAVEPOINT child")?;
        s.execute("DELETE FROM bank.public.accounts WHERE id=2")?;
        s.execute("ROLLBACK TO SAVEPOINT work")?;
        s.execute("INSERT INTO bank.public.messages(id,body) VALUES(2,'keep')")?;
        s.execute("RELEASE work")?;
        assert_eq!(s.snapshot_lsn(), Some(baseline));
        assert_eq!(db.last_lsn(), baseline);
        s.execute("COMMIT")?;
        assert_eq!(s.last_commit_lsn(), Some(baseline + 1));
        Ok(())
    }).unwrap();
    assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Int64(110)]]);
    assert_eq!(query(&db, "SELECT balance FROM bank.public.audit WHERE id=9"), vec![vec![Value::Int64(90)]]);
    assert_eq!(query(&db, "SELECT id FROM bank.public.messages"), vec![vec![Value::Int64(2)]]);
    assert_eq!(query(&db, &format!("SELECT balance FROM bank.public.accounts AS OF {baseline} WHERE id=1")), vec![vec![Value::Int64(100)]]);
    db.checkpoint().unwrap();
    drop(db);
    let db = HistorySqlEngine::open(&root).unwrap();
    assert_eq!(query(&db, "SELECT id FROM bank.public.accounts"), vec![vec![Value::Int64(1)], vec![Value::Int64(2)]]);
}

#[test]
fn failed_statements_require_explicit_rollback_to_before_commit_or_more_work() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    for failure in ["INSERT INTO bank.public.accounts(id,balance) VALUES(1,999)",
        "UPDATE bank.public.accounts SET balance=NULL WHERE id=1", "SELECT nonsense FROM bank.public.accounts", "garbage !!!"] {
        let baseline = db.last_lsn();
        db.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute("UPDATE bank.public.accounts SET balance=120 WHERE id=1")?;
            s.execute("SAVEPOINT recover")?;
            s.execute("UPDATE bank.public.audit SET balance=999 WHERE id=9")?;
            assert!(s.execute(failure).is_err());
            assert!(s.in_transaction());
            for blocked in ["COMMIT", "SAVEPOINT newer", "RELEASE recover", "SELECT * FROM bank.public.accounts", "BEGIN"] {
                assert!(s.execute(blocked).is_err());
            }
            assert_eq!(db.last_lsn(), baseline);
            assert!(s.execute("ROLLBACK TO missing").is_err());
            assert!(s.execute("COMMIT").is_err());
            s.execute("ROLLBACK TO recover")?;
            s.execute("COMMIT")?;
            Ok(())
        }).unwrap();
        assert_eq!(query(&db, "SELECT balance FROM bank.public.audit WHERE id=9"), vec![vec![Value::Int64(90)]]);
    }
}

#[test]
fn duplicate_names_shadow_and_release_preserves_staged_values() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("SAVEPOINT same")?;
        s.execute("UPDATE bank.public.accounts SET balance=130 WHERE id=1")?;
        s.execute("SAVEPOINT same")?;
        s.execute("UPDATE bank.public.accounts SET balance=140 WHERE id=1")?;
        s.execute("ROLLBACK TO same")?;
        assert!(matches!(s.execute("SELECT balance FROM bank.public.accounts WHERE id=1")?,
            SqlResultSet::Rows { rows, .. } if rows == vec![vec![Value::Int64(130)]]));
        s.execute("RELEASE SAVEPOINT same")?;
        s.execute("ROLLBACK TO same")?;
        s.execute("UPDATE bank.public.accounts SET balance=150 WHERE id=1")?;
        s.execute("RELEASE same")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Int64(150)]]);
}

#[test]
fn limits_missing_markers_and_no_marker_errors_leave_no_partial_commit() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    let baseline = db.last_lsn();
    db.with_session(|s| {
        for sql in ["SAVEPOINT x", "RELEASE x", "ROLLBACK TO x"] { assert!(s.execute(sql).is_err()); }
        s.execute("BEGIN")?;
        for i in 0..16 { s.execute(&format!("SAVEPOINT s{i}"))?; }
        assert!(s.execute("SAVEPOINT overflow").is_err());
        s.execute("ROLLBACK TO s0")?;
        s.execute("UPDATE bank.public.accounts SET balance=999 WHERE id=1")?;
        assert!(s.execute("RELEASE absent").is_err());
        s.execute("ROLLBACK")?;
        s.execute("BEGIN")?;
        s.execute("SAVEPOINT final")?;
        s.execute("RELEASE final")?;
        s.execute("UPDATE bank.public.audit SET balance=999 WHERE id=9")?;
        assert!(s.execute("ROLLBACK TO final").is_err());
        assert!(!s.in_transaction());
        assert!(s.execute("COMMIT").is_err());
        Ok(())
    }).unwrap();
    assert_eq!(db.last_lsn(), baseline);
    assert_eq!(query(&db, "SELECT balance FROM bank.public.audit WHERE id=9"), vec![vec![Value::Int64(90)]]);
}

#[test]
fn rollback_to_preserves_sql_snapshot_and_concurrent_write_conflicts() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    for scan in [false, true] {
        db.with_session(|s| {
            s.execute("BEGIN")?;
            let snapshot = s.snapshot_lsn();
            s.execute("SAVEPOINT original")?;
            if scan { s.execute("SELECT * FROM bank.public.accounts")?; }
            modify(&db, &["UPDATE bank.public.accounts SET balance=160 WHERE id=1"]);
            s.execute("ROLLBACK TO original")?;
            assert_eq!(s.snapshot_lsn(), snapshot);
            s.execute(if scan { "UPDATE bank.public.audit SET balance=170 WHERE id=9" }
                else { "UPDATE bank.public.accounts SET balance=170 WHERE id=1" })?;
            assert!(s.execute("COMMIT").is_err());
            assert!(!s.in_transaction());
            Ok(())
        }).unwrap();
    }
    assert_eq!(query(&db, "SELECT balance FROM bank.public.audit WHERE id=9"), vec![vec![Value::Int64(90)]]);
}

#[test]
fn savepoints_compose_with_required_defaults_and_callback_drop() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD state TEXT NOT NULL DEFAULT 'active'").unwrap();
    let baseline = db.last_lsn();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("SAVEPOINT empty")?;
        s.execute("INSERT INTO bank.public.accounts(id,balance) VALUES(3,300)")?;
        s.execute("ROLLBACK TO empty")?;
        s.execute("INSERT INTO bank.public.accounts(id,balance,state) VALUES(4,400,DEFAULT)")?;
        Ok(()) // scope drop must discard every staged write, including marked ones
    }).unwrap();
    assert_eq!(db.last_lsn(), baseline);
    modify(&db, &["SAVEPOINT before_insert", "INSERT INTO bank.public.accounts(id,balance) VALUES(5,500)", "RELEASE before_insert"]);
    assert_eq!(query(&db, "SELECT state FROM bank.public.accounts WHERE id=5"), vec![vec![Value::String("active".into())]]);
    assert!(query(&db, "SELECT id FROM bank.public.accounts WHERE id=4").is_empty());
}
