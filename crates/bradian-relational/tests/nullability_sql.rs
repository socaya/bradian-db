//! G39: constraint relaxation, implicit NULL inserts and historical compatibility.
#[path = "support/epoch_fixture.rs"]
mod fixture;
use fixture::{migrated, modify, query};
use bradian_relational::{Column, DataType, Table};
use bradian_relational::durable_mvcc::DurableMvccStore;
use bradian_relational::history_backup::restore_history_database;
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::sql::SqlResultSet;
use bradian_relational::typed_table::Value;

#[test]
fn relaxation_accepts_null_and_keeps_old_as_of_schema_and_values() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    let old = db.last_lsn();
    assert!(db.drop_not_null("bank.public.accounts", "balance").is_err());
    assert_eq!(db.last_lsn(), old);
    db.enable_epoch_row_writes().unwrap();
    let receipt = db.execute_ddl("ALTER TABLE bank.public.accounts ALTER COLUMN balance DROP NOT NULL;").unwrap();
    let written = modify(&db, &["UPDATE bank.public.accounts SET balance=NULL WHERE id=1"]);
    assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Null]]);
    assert_eq!(query(&db, &format!("SELECT balance FROM bank.public.accounts AS OF {old} WHERE id=1")), vec![vec![Value::Int64(100)]]);
    assert!(!db.schema_as_of("bank.public.accounts", old).unwrap().0.columns[1].nullable);
    assert!(db.schema_as_of("bank.public.accounts", receipt.schema_epoch).unwrap().0.columns[1].nullable);
    db.with_session(|s| {
        let result = s.execute(&format!("DESCRIBE TABLE bank.public.accounts AS OF {old}"))?;
        assert!(matches!(result, SqlResultSet::Rows { rows, .. } if rows[1][2] == Value::Boolean(false)));
        let result = s.execute(&format!("DESCRIBE TABLE bank.public.accounts AS OF {written}"))?;
        assert!(matches!(result, SqlResultSet::Rows { rows, .. } if rows[1][2] == Value::Boolean(true)));
        Ok(())
    }).unwrap();
    db.checkpoint().unwrap();
    drop(db);
    let db = HistorySqlEngine::open(&root).unwrap();
    assert_eq!(db.last_lsn(), written);
    assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Null]]);
    assert_eq!(query(&db, &format!("SELECT balance FROM bank.public.accounts AS OF {old} WHERE id=1")), vec![vec![Value::Int64(100)]]);
}

#[test]
fn omitted_nullable_columns_are_null_and_missing_required_values_abort_sql_work() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD memo TEXT").unwrap();
    modify(&db, &["INSERT INTO bank.public.accounts(id,balance) VALUES(3,300)"]);
    assert_eq!(query(&db, "SELECT memo FROM bank.public.accounts WHERE id=3"), vec![vec![Value::Null]]);
    db.drop_not_null("bank.public.accounts", "BALANCE").unwrap();
    modify(&db, &["INSERT INTO bank.public.accounts(id) VALUES(4)"]);
    assert_eq!(query(&db, "SELECT balance,memo FROM bank.public.accounts WHERE id=4"), vec![vec![Value::Null, Value::Null]]);
    for bad in [
        "INSERT INTO bank.public.accounts(balance) VALUES(500)",
        "INSERT INTO bank.public.audit(id) VALUES(10)",
        "INSERT INTO bank.public.accounts(id,id) VALUES(5,6)",
        "INSERT INTO bank.public.accounts(id,unknown) VALUES(5,6)",
        "INSERT INTO bank.public.accounts(id,balance) VALUES(5)",
        "INSERT INTO bank.public.accounts(id) VALUES(5,6)",
    ] {
        let before = db.last_lsn();
        db.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute("UPDATE bank.public.accounts SET balance=999 WHERE id=1")?;
            assert!(s.execute(bad).is_err(), "accepted {bad}");
            assert!(!s.in_transaction());
            assert!(s.execute("COMMIT").is_err());
            Ok(())
        }).unwrap();
        assert_eq!(db.last_lsn(), before);
        assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Int64(100)]]);
    }
}

#[test]
fn invalid_relaxations_noops_and_narrowing_do_not_advance_wal() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD memo TEXT").unwrap();
    let before = db.last_lsn();
    for sql in [
        "ALTER TABLE bank.public.accounts ALTER COLUMN id DROP NOT NULL",
        "ALTER TABLE bank.public.accounts ALTER COLUMN missing DROP NOT NULL",
        "ALTER TABLE bank.public.accounts ALTER COLUMN memo DROP NOT NULL",
        "ALTER TABLE bank.public.accounts ALTER COLUMN balance SET NOT NULL",
        "ALTER TABLE bank.public.accounts ALTER COLUMN balance DROP NULL",
        "ALTER TABLE bank.public.accounts ALTER COLUMN balance TYPE TEXT",
        "ALTER TABLE bank.public.accounts ALTER COLUMN balance DROP NOT NULL DEFAULT 0",
        "ALTER TABLE bank.public.accounts ALTER COLUMN balance DROP NOT NULL; DELETE FROM bank.public.accounts WHERE id=1",
    ] {
        assert!(db.execute_ddl(sql).is_err(), "accepted {sql}");
        assert_eq!(db.last_lsn(), before);
        assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Int64(100)]]);
    }
    db.execute_ddl("ALTER TABLE bank.public.accounts ALTER balance DROP NOT NULL").unwrap();
    let changed = db.last_lsn();
    assert!(db.drop_not_null("bank.public.accounts", "balance").is_err());
    assert_eq!(db.last_lsn(), changed);
}

#[test]
fn mixed_additions_and_relaxations_project_old_epochs_and_support_multi_table_commits() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    let old = db.last_lsn();
    db.enable_epoch_row_writes().unwrap();
    let g35 = modify(&db, &["UPDATE bank.public.accounts SET balance=101 WHERE id=1"]);
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD memo TEXT").unwrap();
    let added = modify(&db, &["UPDATE bank.public.accounts SET memo='old' WHERE id=1"]);
    db.execute_ddl("ALTER TABLE bank.public.accounts ALTER balance DROP NOT NULL").unwrap();
    let relaxed = modify(&db, &[
        "UPDATE bank.public.accounts SET balance=NULL WHERE id=1",
        "UPDATE bank.public.audit SET balance=91 WHERE id=9",
    ]);
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD score INTEGER").unwrap();
    modify(&db, &["INSERT INTO bank.public.accounts(id,score) VALUES(3,7)"]);
    assert_eq!(query(&db, "SELECT * FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Int64(1), Value::Null, Value::String("old".into()), Value::Null]]);
    assert_eq!(query(&db, "SELECT * FROM bank.public.accounts WHERE id=3"), vec![vec![Value::Int64(3), Value::Null, Value::Null, Value::Int32(7)]]);
    assert_eq!(query(&db, &format!("SELECT * FROM bank.public.accounts AS OF {g35} WHERE id=1")), vec![vec![Value::Int64(1), Value::Int64(101)]]);
    assert_eq!(query(&db, &format!("SELECT * FROM bank.public.accounts AS OF {added} WHERE id=1")), vec![vec![Value::Int64(1), Value::Int64(101), Value::String("old".into())]]);
    assert_eq!(query(&db, &format!("SELECT balance FROM bank.public.accounts AS OF {relaxed} WHERE id=1")), vec![vec![Value::Null]]);
    assert_eq!(query(&db, &format!("SELECT balance FROM bank.public.accounts AS OF {old} WHERE id=1")), vec![vec![Value::Int64(100)]]);
    drop(db);
    let db = HistorySqlEngine::open(&root).unwrap();
    assert_eq!(query(&db, "SELECT balance FROM bank.public.audit WHERE id=9"), vec![vec![Value::Int64(91)]]);
}

#[test]
fn backups_and_retention_preserve_relaxation_and_implicit_null_rows() {
    let (dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    let old = db.last_lsn();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ALTER balance DROP NOT NULL").unwrap();
    modify(&db, &["UPDATE bank.public.accounts SET balance=NULL WHERE id=1"]);
    let floor = modify(&db, &["INSERT INTO bank.public.accounts(id) VALUES(3)"]);
    db.backup_to(dir.path().join("archive")).unwrap();
    restore_history_database(dir.path().join("archive"), dir.path().join("restored")).unwrap();
    let restored = HistorySqlEngine::open(dir.path().join("restored")).unwrap();
    assert_eq!(query(&restored, "SELECT balance FROM bank.public.accounts WHERE id=3"), vec![vec![Value::Null]]);
    assert!(!restored.schema_as_of("bank.public.accounts", old).unwrap().0.columns[1].nullable);
    drop(db);
    let raw = DurableMvccStore::open(root.join("history-domain")).unwrap();
    raw.prune_through(floor).unwrap();
    raw.checkpoint().unwrap();
    drop(raw);
    let mut db = HistorySqlEngine::open(&root).unwrap();
    assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Null]]);
    assert!(db.schema_as_of("bank.public.accounts", old).is_err());
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD memo TEXT").unwrap();
}

#[test]
fn relaxation_does_not_rewrite_stored_rows_or_original_schema_pins() {
    let (_dir, root) = migrated();
    let original = {
        let raw = DurableMvccStore::open(root.join("history-domain")).unwrap();
        raw.scan_as_of(raw.latest_lsn()).unwrap()
    };
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    let (table, _) = db.schema_as_of("bank.public.accounts", db.last_lsn()).unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ALTER balance DROP NOT NULL").unwrap();
    drop(db);
    let raw = DurableMvccStore::open(root.join("history-domain")).unwrap();
    let current = raw.scan_as_of(raw.latest_lsn()).unwrap();
    for (key, value) in original {
        assert!(current.contains(&(key, value)));
    }
    let pin = current.iter().find_map(|(key, bytes)| {
        if !key.starts_with(b"\0bradian-domain-table-v1:") { return None; }
        let pinned: Table = serde_json::from_slice(bytes).unwrap();
        (pinned.id == table.id).then_some(pinned)
    }).unwrap();
    assert!(!pin.columns[1].nullable);
}

#[test]
fn retired_legacy_proposals_can_be_followed_by_relaxation() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    let (mut proposed, _) = db.schema_as_of("bank.public.accounts", db.last_lsn()).unwrap();
    proposed.columns.push(Column { name: "old".into(), data_type: DataType::Text, nullable: true });
    db.prepare_nullable_addition("bank.public.accounts", &proposed).unwrap();
    db.coordinate_prepared_schema("bank.public.accounts").unwrap();
    db.certify_schema_readiness("bank.public.accounts").unwrap();
    db.retire_legacy_schema_intents().unwrap();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ALTER balance DROP NOT NULL").unwrap();
    modify(&db, &["INSERT INTO bank.public.accounts(id) VALUES(3)"]);
    drop(db);
    let db = HistorySqlEngine::open(&root).unwrap();
    assert!(db.legacy_schema_intents_retired());
    assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=3"), vec![vec![Value::Null]]);
    assert_eq!(db.schema_as_of("bank.public.accounts", db.last_lsn()).unwrap().0.columns.len(), 2);
}
