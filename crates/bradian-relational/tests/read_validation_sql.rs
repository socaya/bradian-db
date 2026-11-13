#[path = "support/epoch_fixture.rs"]
mod fixture;
use fixture::{migrated, modify, query};
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::sql::SqlResultSet;
use bradian_relational::typed_table::Value;

#[test]
fn validated_cross_table_point_reads_reject_write_skew_and_default_begin_stays_compatible() {
    for validate in [false, true] {
        let (_dir, root) = migrated();
        let db = HistorySqlEngine::open(&root).unwrap();
        let before = db.last_lsn();
        let begin = if validate { "BEGIN TRANSACTION WITH READ VALIDATION" } else { "BEGIN" };
        db.with_session(|first| {
            first.execute(begin)?;
            assert_eq!(first.read_validation_enabled(), validate);
            first.execute("SELECT balance FROM bank.public.audit WHERE id=9")?;
            first.execute("UPDATE bank.public.accounts SET balance=0 WHERE id=1")?;
            db.with_session(|second| {
                second.execute(begin)?;
                second.execute("SELECT balance FROM bank.public.accounts WHERE id=1")?;
                second.execute("UPDATE bank.public.audit SET balance=0 WHERE id=9")?;
                first.execute("COMMIT")?;
                let result = second.execute("COMMIT");
                assert_eq!(result.is_err(), validate);
                assert!(!second.in_transaction());
                assert!(!second.read_validation_enabled());
                Ok(())
            })?;
            Ok(())
        }).unwrap();
        assert_eq!(db.last_lsn(), before + if validate { 1 } else { 2 });
        assert_eq!(db.mvcc_stats().active_snapshots, 0);
        assert_eq!(query(&db, "SELECT balance FROM bank.public.audit WHERE id=9"),
            vec![vec![Value::Int64(if validate { 90 } else { 0 })]]);
        db.checkpoint().unwrap();
        drop(db);
        let db = HistorySqlEngine::open(&root).unwrap();
        assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Int64(0)]]);
    }
}

#[test]
fn read_only_and_absent_row_observations_fail_without_partial_writes() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    for write in [false, true] {
        let id = if write { 4 } else { 3 };
        db.with_session(|s| {
            s.execute("BEGIN WITH READ VALIDATION")?;
            assert!(matches!(s.execute(&format!("SELECT id FROM bank.public.accounts WHERE id={id}"))?,
                SqlResultSet::Rows { rows, .. } if rows.is_empty()));
            modify(&db, &[&format!("INSERT INTO bank.public.accounts(id,balance) VALUES({id},300)")]);
            let latest = db.last_lsn();
            if write { s.execute("UPDATE bank.public.audit SET balance=999 WHERE id=9")?; }
            assert!(s.execute("COMMIT").is_err());
            assert!(!s.in_transaction());
            assert_eq!(db.last_lsn(), latest);
            Ok(())
        }).unwrap();
    }
    assert_eq!(query(&db, "SELECT balance FROM bank.public.audit WHERE id=9"), vec![vec![Value::Int64(90)]]);
    assert_eq!(db.mvcc_stats().active_snapshots, 0);
}

#[test]
fn savepoint_error_recovery_cannot_erase_reads_or_disable_validation() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN WITH READ VALIDATION")?;
        let snapshot = s.snapshot_lsn();
        s.execute("SAVEPOINT before_read")?;
        s.execute("SELECT balance FROM bank.public.accounts WHERE id=1")?;
        assert!(s.execute("UPDATE bank.public.accounts SET balance=NULL WHERE id=1").is_err());
        assert!(s.execute("COMMIT").is_err());
        s.execute("ROLLBACK TO before_read")?;
        assert!(s.read_validation_enabled());
        assert_eq!(s.snapshot_lsn(), snapshot);
        modify(&db, &["UPDATE bank.public.accounts SET balance=190 WHERE id=1"]);
        s.execute("RELEASE before_read")?;
        s.execute("UPDATE bank.public.audit SET balance=999 WHERE id=9")?;
        let latest = db.last_lsn();
        assert!(s.execute("COMMIT").is_err());
        assert_eq!(db.last_lsn(), latest);
        s.execute("BEGIN")?;
        assert!(!s.read_validation_enabled());
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(query(&db, "SELECT balance FROM bank.public.audit WHERE id=9"), vec![vec![Value::Int64(90)]]);
}

#[test]
fn scan_phantoms_validate_even_after_rollback_to_an_earlier_marker() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN WITH READ VALIDATION")?;
        s.execute("SAVEPOINT before_scan")?;
        s.execute("SELECT * FROM bank.public.messages")?;
        s.execute("ROLLBACK TO before_scan")?;
        modify(&db, &["INSERT INTO bank.public.messages(id,body) VALUES(1,'phantom')"]);
        assert!(s.execute("COMMIT").is_err());
        assert!(!s.in_transaction());
        Ok(())
    }).unwrap();
}

#[test]
fn historical_reads_and_invalid_mode_syntax_cannot_silently_bypass_validation() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    let before = db.last_lsn();
    db.with_session(|s| {
        for begin in ["BEGIN WITH READ", "BEGIN WITH VALIDATION", "BEGIN WITH READ VALIDATION extra",
            "BEGIN ISOLATION LEVEL SERIALIZABLE"] {
            assert!(s.execute(begin).is_err());
            assert!(!s.in_transaction());
        }
        s.execute("BEGIN WITH READ VALIDATION")?;
        s.execute("SAVEPOINT safe")?;
        for sql in [format!("SELECT * FROM bank.public.accounts AS OF {before}"),
            format!("SELECT balance FROM bank.public.accounts AS OF {before} WHERE id=1"),
            format!("DESCRIBE TABLE bank.public.accounts AS OF {before} WITH DEFAULTS")] {
            assert!(s.execute(&sql).is_err());
            assert!(s.execute("COMMIT").is_err());
            s.execute("ROLLBACK TO safe")?;
            assert!(s.read_validation_enabled());
        }
        s.execute("COMMIT")?;
        assert!(!s.read_validation_enabled());
        Ok(())
    }).unwrap();
    assert_eq!(db.last_lsn(), before);
    assert_eq!(query(&db, &format!("SELECT balance FROM bank.public.accounts AS OF {before} WHERE id=1")), vec![vec![Value::Int64(100)]]);
}

#[test]
fn unrelated_validated_transactions_and_defaulted_inserts_commit_independently() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD state TEXT NOT NULL DEFAULT 'active'").unwrap();
    db.with_session(|first| {
        first.execute("BEGIN WITH READ VALIDATION")?;
        first.execute("INSERT INTO bank.public.accounts(id,balance) VALUES(3,300)")?;
        db.with_session(|second| {
            second.execute("BEGIN WITH READ VALIDATION")?;
            second.execute("INSERT INTO bank.public.messages(id,body) VALUES(1,'independent')")?;
            first.execute("COMMIT")?;
            second.execute("COMMIT")?;
            Ok(())
        })?;
        Ok(())
    }).unwrap();
    assert_eq!(query(&db, "SELECT state FROM bank.public.accounts WHERE id=3"), vec![vec![Value::String("active".into())]]);
    assert_eq!(query(&db, "SELECT body FROM bank.public.messages WHERE id=1"), vec![vec![Value::String("independent".into())]]);
    let before = db.last_lsn();
    db.with_session(|s| {
        s.execute("BEGIN WITH READ VALIDATION")?;
        s.execute("SELECT * FROM bank.public.accounts")?;
        Ok(())
    }).unwrap();
    assert_eq!(db.last_lsn(), before);
    assert_eq!(db.mvcc_stats().active_snapshots, 0);
}
