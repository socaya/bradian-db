#[path = "support/epoch_fixture.rs"]
mod fixture;
use fixture::{migrated, modify, query};
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::history_backup::restore_history_database;
use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::typed_table::Value;

fn ids(rows: Vec<Vec<Value>>) -> Vec<i64> {
    rows.into_iter().map(|row| match row[0] { Value::Int64(id) => id, ref other => panic!("unexpected id: {other:?}") }).collect()
}

#[test]
fn typed_sorting_null_placement_ties_and_unprojected_fields_have_golden_order() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    for clause in ["score INTEGER", "price NUMERIC(8,2)", "label TEXT", "flag BOOLEAN"] {
        db.execute_ddl(&format!("ALTER TABLE bank.public.accounts ADD {clause}")).unwrap();
    }
    modify(&db, &["INSERT INTO bank.public.accounts(id,balance,score,price,label,flag) VALUES(-1,100,-10,-1.25,'Z',false)",
        "INSERT INTO bank.public.accounts(id,balance,score,price,label,flag) VALUES(0,100,2,2.00,'a',true)",
        "UPDATE bank.public.accounts SET score=2,price=2.00,label='é',flag=true WHERE id=1"]);
    for (order, expected) in [
        ("score", vec![-1,0,1,2]), ("score DESC", vec![2,0,1,-1]),
        ("score DESC NULLS LAST", vec![0,1,-1,2]), ("score ASC NULLS FIRST", vec![2,-1,0,1]),
        ("score ASC, label DESC", vec![-1,1,0,2]),
        ("price ASC", vec![-1,0,1,2]), ("label", vec![-1,0,1,2]),
        ("flag DESC NULLS LAST, id DESC", vec![1,0,-1,2]),
    ] {
        assert_eq!(ids(query(&db, &format!("SELECT id FROM bank.public.accounts ORDER BY {order}"))), expected, "{order}");
    }
}

#[test]
fn logical_primary_key_pagination_is_stable_in_one_snapshot_during_concurrent_writes() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    modify(&db, &["INSERT INTO bank.public.accounts(id,balance) VALUES(-10,100)",
        "INSERT INTO bank.public.accounts(id,balance) VALUES(-1,100)",
        "INSERT INTO bank.public.accounts(id,balance) VALUES(0,100)",
        "INSERT INTO bank.public.accounts(id,balance) VALUES(3,100)"]);
    db.with_session(|s| {
        s.execute("BEGIN")?;
        let mut combined = Vec::new();
        for offset in [0,2,4] {
            if offset == 2 { modify(&db, &["INSERT INTO bank.public.accounts(id,balance) VALUES(-20,100)"]); }
            match s.execute(&format!("SELECT id FROM bank.public.accounts LIMIT 2 OFFSET {offset}"))? {
                SqlResultSet::Rows { rows, .. } => combined.extend(ids(rows)), other => panic!("{other:?}"),
            }
        }
        assert_eq!(combined, vec![-10,-1,0,1,2,3]);
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(ids(query(&db, "SELECT id FROM bank.public.accounts OFFSET 5")), vec![2,3]);
    assert!(query(&db, "SELECT id FROM bank.public.accounts LIMIT 0").is_empty());
    assert!(query(&db, "SELECT id FROM bank.public.accounts LIMIT 1 OFFSET 1000000").is_empty());
    assert_eq!(ids(query(&db, "SELECT id FROM bank.public.accounts WHERE id=-10 ORDER BY balance DESC LIMIT 1")), vec![-10]);
    assert!(query(&db, "SELECT id FROM bank.public.accounts WHERE id=-10 OFFSET 1").is_empty());
}

#[test]
fn as_of_sort_columns_bind_to_historical_schemas_and_immutable_defaults() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    let original = db.last_lsn();
    db.enable_epoch_row_writes().unwrap();
    let epoch = db.execute_ddl("ALTER TABLE bank.public.accounts ADD priority INTEGER NOT NULL DEFAULT 5").unwrap().schema_epoch;
    modify(&db, &["UPDATE bank.public.accounts SET priority=1 WHERE id=2"]);
    assert_eq!(ids(query(&db, &format!("SELECT id FROM bank.public.accounts AS OF {epoch} ORDER BY priority LIMIT 1"))), vec![1]);
    assert_eq!(ids(query(&db, "SELECT id FROM bank.public.accounts ORDER BY priority LIMIT 1")), vec![2]);
    assert_eq!(ids(query(&db, &format!("SELECT id FROM bank.public.accounts AS OF {original} ORDER BY balance DESC LIMIT 1"))), vec![2]);
    db.with_session(|s| {
        s.execute("BEGIN")?;
        assert!(s.execute(&format!("SELECT id FROM bank.public.accounts AS OF {original} ORDER BY priority")).is_err());
        Ok(())
    }).unwrap();
}

#[test]
fn windows_see_own_staged_mutations_and_savepoint_restore() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    let before = db.last_lsn();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("SAVEPOINT original")?;
        s.execute("INSERT INTO bank.public.accounts(id,balance) VALUES(3,500)")?;
        s.execute("UPDATE bank.public.accounts SET balance=balance+1000 WHERE id=1")?;
        s.execute("DELETE FROM bank.public.accounts WHERE id=2")?;
        assert!(matches!(s.execute("SELECT id FROM bank.public.accounts ORDER BY balance DESC LIMIT 2")?,
            SqlResultSet::Rows { rows, .. } if ids(rows.clone()) == vec![1,3]));
        s.execute("ROLLBACK TO original")?;
        assert!(matches!(s.execute("SELECT id FROM bank.public.accounts ORDER BY balance DESC LIMIT 2")?,
            SqlResultSet::Rows { rows, .. } if ids(rows.clone()) == vec![2,1]));
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    assert_eq!(db.last_lsn(), before);
}

#[test]
fn invalid_order_and_window_clauses_abort_or_require_explicit_savepoint_recovery() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    let before = db.last_lsn();
    for clause in ["ORDER BY missing", "ORDER BY id,id", "ORDER BY 1", "ORDER BY balance+1", "ORDER BY id ASC DESC",
        "ORDER BY id NULLS", "ORDER BY id NULLS middle", "ORDER BY id COLLATE x", "ORDER id",
        "LIMIT -1", "LIMIT 1.5", "LIMIT 10001", "LIMIT 18446744073709551616", "LIMIT ALL",
        "OFFSET -1", "OFFSET 1000001", "OFFSET 1 LIMIT 2", "LIMIT 1 LIMIT 2", "LIMIT 2 ORDER BY id",
        "ORDER BY id; DELETE FROM bank.public.audit WHERE id=9"] {
        db.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute("UPDATE bank.public.audit SET balance=999 WHERE id=9")?;
            assert!(s.execute(&format!("SELECT id FROM bank.public.accounts {clause}")).is_err(), "accepted {clause}");
            assert!(!s.in_transaction());
            assert!(s.execute("COMMIT").is_err());
            Ok(())
        }).unwrap();
        assert_eq!(db.last_lsn(), before);
    }
    db.with_session(|s| {
        s.execute("BEGIN WITH READ VALIDATION")?;
        s.execute("SAVEPOINT before_parse")?;
        assert!(s.execute("SELECT id FROM bank.public.accounts ORDER BY missing").is_err());
        assert!(s.execute("COMMIT").is_err());
        s.execute("ROLLBACK TO before_parse")?;
        // Invalid binding must not have scanned and over-fenced unrelated writes.
        modify(&db, &["UPDATE bank.public.accounts SET balance=201 WHERE id=2"]);
        s.execute("UPDATE bank.public.audit SET balance=91 WHERE id=9")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
}

#[test]
fn limited_scans_including_limit_zero_preserve_read_validation_conflicts() {
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    for (limit,id) in [(0,3),(1,4)] {
        db.with_session(|s| {
            s.execute("BEGIN WITH READ VALIDATION")?;
            s.execute("SAVEPOINT before_scan")?;
            s.execute(&format!("SELECT id FROM bank.public.accounts ORDER BY balance DESC LIMIT {limit}"))?;
            s.execute("ROLLBACK TO before_scan")?;
            modify(&db, &[&format!("INSERT INTO bank.public.accounts(id,balance) VALUES({id},500)")]);
            assert!(s.execute("COMMIT").is_err());
            assert!(!s.in_transaction());
            Ok(())
        }).unwrap();
    }
    db.with_session(|s| {
        s.execute("BEGIN WITH READ VALIDATION")?;
        s.execute("SELECT id FROM bank.public.accounts WHERE id=1 ORDER BY balance LIMIT 1")?;
        modify(&db, &["UPDATE bank.public.accounts SET balance=202 WHERE id=2"]);
        s.execute("UPDATE bank.public.audit SET balance=91 WHERE id=9")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    assert_eq!(db.mvcc_stats().active_snapshots, 0);
}

#[test]
fn query_input_and_sort_key_budgets_reject_without_early_truncation() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    for i in 0..15 { db.execute_ddl(&format!("ALTER TABLE bank.public.accounts ADD c{i} BIGINT")).unwrap(); }
    let sixteen = std::iter::once("id".to_string()).chain((0..15).map(|i|format!("c{i}"))).collect::<Vec<_>>().join(",");
    assert_eq!(ids(query(&db, &format!("SELECT id FROM bank.public.accounts ORDER BY {sixteen}"))), vec![1,2]);
    db.with_session(|s| {
        s.execute("BEGIN")?;
        assert!(s.execute(&format!("SELECT id FROM bank.public.accounts ORDER BY {sixteen},balance")).is_err());
        Ok(())
    }).unwrap();
    // Build exactly 10000 rows, then exceed the budget by one. Small batches
    // obey the existing 256-write-key transaction capacity.
    for start in (3..=10000).step_by(250) {
        let statements = (start..(start+250).min(10001)).map(|id|
            format!("INSERT INTO bank.public.accounts(id,balance) VALUES({id},{id})")).collect::<Vec<_>>();
        modify(&db, &statements.iter().map(String::as_str).collect::<Vec<_>>());
    }
    assert_eq!(ids(query(&db, "SELECT id FROM bank.public.accounts ORDER BY id DESC LIMIT 1")), vec![10000]);
    modify(&db, &["INSERT INTO bank.public.accounts(id,balance) VALUES(10001,10001)"]);
    let before = db.last_lsn();
    for suffix in ["ORDER BY id DESC LIMIT 1", "LIMIT 0", "OFFSET 1000000"] {
        db.with_session(|s| {
            s.execute("BEGIN")?;
            assert!(s.execute(&format!("SELECT id FROM bank.public.accounts {suffix}")).is_err());
            Ok(())
        }).unwrap();
    }
    assert_eq!(db.last_lsn(), before);
    assert_eq!(query(&db, "SELECT id FROM bank.public.accounts").len(), 10001);
}

#[test]
fn ordering_and_page_results_survive_checkpoint_and_verified_restore() {
    let (dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    modify(&db, &["INSERT INTO bank.public.accounts(id,balance) VALUES(-1,100)"]);
    let latest = db.last_lsn();
    let sql = format!("SELECT id FROM bank.public.accounts AS OF {latest} ORDER BY balance DESC LIMIT 2 OFFSET 1");
    let expected = query(&db, &sql);
    assert_eq!(ids(expected.clone()), vec![-1,1]);
    db.checkpoint().unwrap();
    let archive = dir.path().join("window-backup");
    db.backup_to(&archive).unwrap();
    drop(db);
    let restored = dir.path().join("restored");
    restore_history_database(&archive, &restored).unwrap();
    let db = HistorySqlEngine::open(&restored).unwrap();
    assert_eq!(query(&db, &sql), expected);
}

#[test]
fn older_sql_profile_rejects_query_windows_without_changing_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in ["CREATE DATABASE test", "CREATE SCHEMA test.public",
        "CREATE TABLE test.public.items (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO test.public.items(id) VALUES(1)"] { db.execute(sql).unwrap(); }
    for suffix in ["ORDER BY id", "LIMIT 1", "OFFSET 1"] {
        assert!(db.execute(&format!("SELECT id FROM test.public.items {suffix}")).is_err());
    }
    assert!(matches!(db.execute("SELECT id FROM test.public.items").unwrap(), SqlResultSet::Rows { rows, .. } if ids(rows.clone()) == vec![1]));
}
