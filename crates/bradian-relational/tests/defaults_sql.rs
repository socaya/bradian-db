//! G40: epoch-bound constant defaults with required additions and implicit fills.
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
fn required_default_projects_old_rows_and_fills_omitted_or_default_insert_values() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    let old = db.last_lsn();
    db.enable_epoch_row_writes().unwrap();
    let receipt = db.execute_ddl("ALTER TABLE bank.public.accounts ADD score BIGINT NOT NULL DEFAULT 7").unwrap();
    assert_eq!(query(&db, "SELECT score FROM bank.public.accounts"), vec![vec![Value::Int64(7)]; 2]);
    modify(&db, &["INSERT INTO bank.public.accounts(id,balance) VALUES(3,300)",
        "INSERT INTO bank.public.accounts(id,balance,score) VALUES(4,400,DEFAULT)",
        "INSERT INTO bank.public.accounts(id,balance,score) VALUES(5,500,9)"]);
    assert_eq!(query(&db, "SELECT score FROM bank.public.accounts WHERE id=3"), vec![vec![Value::Int64(7)]]);
    assert_eq!(query(&db, "SELECT score FROM bank.public.accounts WHERE id=4"), vec![vec![Value::Int64(7)]]);
    assert_eq!(query(&db, "SELECT score FROM bank.public.accounts WHERE id=5"), vec![vec![Value::Int64(9)]]);
    let (schema, epoch) = db.schema_as_of("bank.public.accounts", receipt.schema_epoch).unwrap();
    assert_eq!(epoch, receipt.schema_epoch);
    assert_eq!(schema.column_defaults.get("score"), Some(&Value::Int64(7)));
    assert!(!schema.columns.last().unwrap().nullable);
    assert!(db.schema_as_of("bank.public.accounts", old).unwrap().0.column_defaults.is_empty());
    assert_eq!(query(&db, &format!("SELECT * FROM bank.public.accounts AS OF {old} WHERE id=1")), vec![vec![Value::Int64(1), Value::Int64(100)]]);
    db.checkpoint().unwrap();
    drop(db);
    let db = HistorySqlEngine::open(&root).unwrap();
    assert_eq!(query(&db, "SELECT score FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Int64(7)]]);
}

#[test]
fn nullable_defaults_distinguish_explicit_null_and_explicit_default() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD memo TEXT DEFAULT 'pending'").unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD absent TEXT DEFAULT NULL").unwrap();
    modify(&db, &["INSERT INTO bank.public.accounts(id,balance,memo) VALUES(3,300,NULL)",
        "INSERT INTO bank.public.accounts(id,balance,memo) VALUES(4,400,DEFAULT)"]);
    assert_eq!(query(&db, "SELECT memo FROM bank.public.accounts WHERE id=1"), vec![vec![Value::String("pending".into())]]);
    assert_eq!(query(&db, "SELECT memo FROM bank.public.accounts WHERE id=3"), vec![vec![Value::Null]]);
    assert_eq!(query(&db, "SELECT memo,absent FROM bank.public.accounts WHERE id=4"), vec![vec![Value::String("pending".into()), Value::Null]]);
    let lsn = db.last_lsn();
    db.with_session(|s| {
        let extended = s.execute(&format!("DESCRIBE TABLE bank.public.accounts AS OF {lsn} WITH DEFAULTS"))?;
        match extended {
            SqlResultSet::Rows { columns, rows } => {
                assert_eq!(&columns[4..], &["has_default", "column_default"]);
                assert_eq!(&rows[0][4..], &[Value::Boolean(false), Value::Null]);
                assert_eq!(&rows[2][4..], &[Value::Boolean(true), Value::String("pending".into())]);
                assert_eq!(&rows[3][4..], &[Value::Boolean(true), Value::Null]);
            }, other => panic!("unexpected metadata: {other:?}"),
        }
        assert!(matches!(s.execute(&format!("DESCRIBE TABLE bank.public.accounts AS OF {lsn}"))?,
            SqlResultSet::Rows { columns, .. } if columns.len() == 4));
        Ok(())
    }).unwrap();
}

#[test]
fn typed_numeric_boolean_text_defaults_and_clause_orders_are_validated() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD enabled BOOLEAN DEFAULT true NOT NULL").unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD amount NUMERIC(38,2) NOT NULL DEFAULT 999999999999999999999999999999999999.99").unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD label VARCHAR(8) DEFAULT 'it''s ok'").unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD adjustment INTEGER DEFAULT -9 NULL").unwrap();
    let expected = vec![Value::Boolean(true), Value::Decimal(10_i128.pow(38) - 1), Value::String("it's ok".into()), Value::Int32(-9)];
    assert_eq!(query(&db, "SELECT enabled,amount,label,adjustment FROM bank.public.accounts WHERE id=1"), vec![expected.clone()]);
    drop(db);
    let db = HistorySqlEngine::open(&root).unwrap();
    assert_eq!(query(&db, "SELECT enabled,amount,label,adjustment FROM bank.public.accounts WHERE id=1"), vec![expected]);
}

#[test]
fn invalid_defaults_and_required_nulls_leave_no_partial_metadata_or_rows() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    let before = db.last_lsn();
    for ddl in [
        "ALTER TABLE bank.public.accounts ADD bad BIGINT NOT NULL DEFAULT NULL",
        "ALTER TABLE bank.public.accounts ADD bad INTEGER DEFAULT 'x'",
        "ALTER TABLE bank.public.accounts ADD bad VARCHAR(2) DEFAULT 'long'",
        "ALTER TABLE bank.public.accounts ADD bad NUMERIC(39,0) DEFAULT 1",
        "ALTER TABLE bank.public.accounts ADD bad NUMERIC(0,0) DEFAULT 1",
        "ALTER TABLE bank.public.accounts ADD bad NUMERIC(4,5) DEFAULT 1",
        "ALTER TABLE bank.public.accounts ADD bad VARCHAR(0) DEFAULT ''",
        "ALTER TABLE bank.public.accounts ADD bad BIGINT DEFAULT 9223372036854775808",
        "ALTER TABLE bank.public.accounts ADD bad BIGINT DEFAULT 1 DEFAULT 2",
        "ALTER TABLE bank.public.accounts ADD bad BIGINT NULL NOT NULL DEFAULT 1",
        "ALTER TABLE bank.public.accounts ADD bad TEXT DEFAULT upper('x')",
        "ALTER TABLE bank.public.accounts ADD bad TEXT NOT NULL",
    ] {
        assert!(db.execute_ddl(ddl).is_err(), "accepted {ddl}");
        assert_eq!(db.last_lsn(), before);
    }
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD score BIGINT DEFAULT 7 NOT NULL").unwrap();
    for insert in ["INSERT INTO bank.public.accounts(id,balance,score) VALUES(3,300,NULL)",
        "INSERT INTO bank.public.accounts(id) VALUES(3)",
        "INSERT INTO bank.public.accounts(id,balance) VALUES(DEFAULT,300)"]
    {
        let before = db.last_lsn();
        db.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute("UPDATE bank.public.accounts SET balance=999 WHERE id=1")?;
            assert!(s.execute(insert).is_err());
            assert!(!s.in_transaction());
            assert!(s.execute("COMMIT").is_err());
            Ok(())
        }).unwrap();
        assert_eq!(db.last_lsn(), before);
        assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Int64(100)]]);
    }
}

#[test]
fn default_epochs_compose_with_relaxation_nullable_additions_and_retention() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    let first = db.execute_ddl("ALTER TABLE bank.public.accounts ADD score BIGINT NOT NULL DEFAULT 7").unwrap();
    modify(&db, &["UPDATE bank.public.accounts SET score=8 WHERE id=1"]);
    db.execute_ddl("ALTER TABLE bank.public.accounts ALTER score DROP NOT NULL").unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD memo TEXT").unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD checked BOOLEAN NOT NULL DEFAULT false").unwrap();
    let floor = modify(&db, &["INSERT INTO bank.public.accounts(id,balance,score) VALUES(3,300,NULL)"]);
    assert_eq!(query(&db, "SELECT score,memo,checked FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Int64(8), Value::Null, Value::Boolean(false)]]);
    assert_eq!(query(&db, "SELECT score FROM bank.public.accounts WHERE id=2"), vec![vec![Value::Int64(7)]]);
    assert_eq!(query(&db, &format!("SELECT score FROM bank.public.accounts AS OF {} WHERE id=1", first.schema_epoch)), vec![vec![Value::Int64(7)]]);
    drop(db);
    let raw = DurableMvccStore::open(root.join("history-domain")).unwrap();
    raw.prune_through(floor).unwrap();
    raw.checkpoint().unwrap();
    drop(raw);
    let db = HistorySqlEngine::open(&root).unwrap();
    assert_eq!(query(&db, "SELECT score,memo,checked FROM bank.public.accounts WHERE id=2"), vec![vec![Value::Int64(7), Value::Null, Value::Boolean(false)]]);
    assert_eq!(query(&db, "SELECT score FROM bank.public.accounts WHERE id=3"), vec![vec![Value::Null]]);
}

#[test]
fn backup_restore_preserves_defaults_pins_and_untouched_row_bytes() {
    let (dir, root) = migrated();
    let before = {
        let raw = DurableMvccStore::open(root.join("history-domain")).unwrap();
        raw.scan_as_of(raw.latest_lsn()).unwrap()
    };
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD note TEXT NOT NULL DEFAULT 'kept'").unwrap();
    db.backup_to(dir.path().join("archive")).unwrap();
    restore_history_database(dir.path().join("archive"), dir.path().join("restored")).unwrap();
    let restored = HistorySqlEngine::open(dir.path().join("restored")).unwrap();
    assert_eq!(query(&restored, "SELECT note FROM bank.public.accounts WHERE id=1"), vec![vec![Value::String("kept".into())]]);
    modify(&restored, &["INSERT INTO bank.public.accounts(id,balance) VALUES(3,300)"]);
    assert_eq!(query(&restored, "SELECT note FROM bank.public.accounts WHERE id=3"), vec![vec![Value::String("kept".into())]]);
    drop(db);
    let raw = DurableMvccStore::open(root.join("history-domain")).unwrap();
    let after = raw.scan_as_of(raw.latest_lsn()).unwrap();
    for pair in before { assert!(after.contains(&pair)); }
    assert!(after.iter().filter(|(key, _)| key.starts_with(b"\0bradian-domain-table-v1:")).all(|(_, value)| {
        let table: Table = serde_json::from_slice(value).unwrap();
        table.column_defaults.is_empty()
    }));
}

#[test]
fn native_default_api_rejects_wrong_values_and_journal_overflow_before_writes() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    let before = db.last_lsn();
    assert!(db.add_column_with_default("bank.public.accounts", Column {
        name: "score".into(), data_type: DataType::Int64, nullable: false,
    }, Value::String("not an integer".into())).is_err());
    assert!(db.add_column_with_default("bank.public.accounts", Column {
        name: "huge".into(), data_type: DataType::Text, nullable: false,
    }, Value::String("x".repeat(4000))).is_err());
    assert_eq!(db.last_lsn(), before);
    db.add_column_with_default("bank.public.accounts", Column {
        name: "score".into(), data_type: DataType::Int64, nullable: false,
    }, Value::Int64(42)).unwrap();
    assert_eq!(query(&db, "SELECT score FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Int64(42)]]);
}
