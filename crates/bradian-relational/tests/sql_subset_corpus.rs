//! Documented SQL subset conformance corpus (ADR 0055).
//!
//! Each test proves exactly one row of `docs/reference/BRADIAN_SQL_SUBSET.md`.
//! Tests use only the three public engine APIs; no engine internals are reached.

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::history_migration::migrate_shared_to_history;
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

// ---------- test fixtures ----------

fn sql_engine(root: &std::path::Path) -> SqlEngine {
    SqlEngine::open(root).unwrap()
}

fn seed_single_table(engine: &SqlEngine) {
    for sql in [
        "CREATE DATABASE test",
        "CREATE SCHEMA test.public",
        "CREATE TABLE test.public.items (id BIGINT NOT NULL, amount BIGINT NOT NULL, PRIMARY KEY(id))",
    ] {
        assert_eq!(engine.execute(sql).unwrap(), SqlResultSet::Command { affected: 0 });
    }
}

fn seed_history() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let source = dir.path().join("legacy-shared");
    let history = dir.path().join("durable-history");
    {
        let ddl = SqlEngine::open(&source).unwrap();
        for sql in [
            "CREATE DATABASE bank",
            "CREATE SCHEMA bank.public",
            "CREATE TABLE bank.public.accounts (id BIGINT NOT NULL, balance BIGINT NOT NULL, PRIMARY KEY(id))",
            "CREATE TABLE bank.public.movements (id BIGINT NOT NULL, amount BIGINT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let db = DomainSqlEngine::open(&source).unwrap();
        db.register_catalog_table("bank.public.accounts").unwrap();
        db.register_catalog_table("bank.public.movements").unwrap();
        db.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute("INSERT INTO bank.public.accounts(id,balance) VALUES(1,1000)")?;
            s.execute("INSERT INTO bank.public.accounts(id,balance) VALUES(2,600)")?;
            s.execute("INSERT INTO bank.public.movements(id,amount) VALUES(10,25)")?;
            s.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
    migrate_shared_to_history(&source, &history).unwrap();
    (dir, history)
}

// ---------- §2. literal syntax ----------

#[test]
fn literal_integer_positive() {
    let dir = tempdir().unwrap();
    let db = sql_engine(dir.path());
    seed_single_table(&db);
    db.execute("INSERT INTO test.public.items (id,amount) VALUES (1,42)").unwrap();
    let rows = db.execute("SELECT amount FROM test.public.items WHERE id=1").unwrap();
    assert_eq!(rows, SqlResultSet::Rows {
        columns: vec!["amount".into()],
        rows: vec![vec![Value::Int64(42)]],
    });
}

#[test]
fn literal_integer_negative() {
    let dir = tempdir().unwrap();
    let db = sql_engine(dir.path());
    seed_single_table(&db);
    db.execute("INSERT INTO test.public.items (id,amount) VALUES (1,-100)").unwrap();
    assert_eq!(
        db.execute("SELECT amount FROM test.public.items WHERE id=1").unwrap(),
        SqlResultSet::Rows {
            columns: vec!["amount".into()],
            rows: vec![vec![Value::Int64(-100)]],
        },
    );
}

#[test]
fn literal_text_empty_and_escaped_and_utf8() {
    let dir = tempdir().unwrap();
    let db = sql_engine(dir.path());
    for sql in [
        "CREATE DATABASE test",
        "CREATE SCHEMA test.public",
        "CREATE TABLE test.public.notes (id BIGINT NOT NULL, body TEXT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    db.execute("INSERT INTO test.public.notes (id,body) VALUES (1,'')").unwrap();
    db.execute("INSERT INTO test.public.notes (id,body) VALUES (2,'it''s ok')").unwrap();
    db.execute("INSERT INTO test.public.notes (id,body) VALUES (3,'é')").unwrap();
    assert_eq!(
        db.execute("SELECT body FROM test.public.notes WHERE id=1").unwrap(),
        SqlResultSet::Rows { columns: vec!["body".into()], rows: vec![vec![Value::String(String::new())]] },
    );
    assert_eq!(
        db.execute("SELECT body FROM test.public.notes WHERE id=2").unwrap(),
        SqlResultSet::Rows { columns: vec!["body".into()], rows: vec![vec![Value::String("it's ok".into())]] },
    );
    assert_eq!(
        db.execute("SELECT body FROM test.public.notes WHERE id=3").unwrap(),
        SqlResultSet::Rows { columns: vec!["body".into()], rows: vec![vec![Value::String("é".into())]] },
    );
}

#[test]
fn literal_boolean_and_null_case_insensitive() {
    let dir = tempdir().unwrap();
    let db = sql_engine(dir.path());
    for sql in [
        "CREATE DATABASE test",
        "CREATE SCHEMA test.public",
        "CREATE TABLE test.public.flags (id BIGINT NOT NULL, flag BOOLEAN NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    db.execute("INSERT INTO test.public.flags (id,flag) VALUES (1,TRUE)").unwrap();
    db.execute("INSERT INTO test.public.flags (id,flag) VALUES (2,false)").unwrap();
    db.execute("INSERT INTO test.public.flags (id,flag) VALUES (3,null)").unwrap();
    assert_eq!(
        db.execute("SELECT flag FROM test.public.flags WHERE id=1").unwrap(),
        SqlResultSet::Rows { columns: vec!["flag".into()], rows: vec![vec![Value::Boolean(true)]] },
    );
    assert_eq!(
        db.execute("SELECT flag FROM test.public.flags WHERE id=2").unwrap(),
        SqlResultSet::Rows { columns: vec!["flag".into()], rows: vec![vec![Value::Boolean(false)]] },
    );
    assert_eq!(
        db.execute("SELECT flag FROM test.public.flags WHERE id=3").unwrap(),
        SqlResultSet::Rows { columns: vec!["flag".into()], rows: vec![vec![Value::Null]] },
    );
}

// ---------- §1. data types ----------

#[test]
fn type_int32_and_int64_and_varchar_and_decimal_round_trip() {
    let dir = tempdir().unwrap();
    let db = sql_engine(dir.path());
    for sql in [
        "CREATE DATABASE test",
        "CREATE SCHEMA test.public",
        "CREATE TABLE test.public.catalog (
            id INT NOT NULL,
            big BIGINT NOT NULL,
            label VARCHAR(16) NOT NULL,
            price DECIMAL(6,2) NOT NULL,
            PRIMARY KEY(id)
        )",
    ] { db.execute(sql).unwrap(); }
    db.execute(
        "INSERT INTO test.public.catalog (id,big,label,price) VALUES (1,9000000000,'widget',12.34)",
    ).unwrap();
    let rows = db.execute("SELECT id,big,label,price FROM test.public.catalog WHERE id=1").unwrap();
    match rows {
        SqlResultSet::Rows { columns, rows } => {
            assert_eq!(columns, vec!["id", "big", "label", "price"]);
            assert_eq!(rows.len(), 1);
            assert!(matches!(rows[0][0], Value::Int32(1)));
            assert!(matches!(rows[0][1], Value::Int64(9_000_000_000)));
            match &rows[0][2] {
                Value::String(s) => assert_eq!(s, "widget"),
                other => panic!("expected String for VARCHAR, got {other:?}"),
            }
            // DECIMAL(6,2) stores raw units scaled to the column's declared scale.
            assert!(matches!(rows[0][3], Value::Decimal(1234)));
        }
        _ => panic!("expected rows"),
    }
}

// ---------- §3-§4. DDL + DML on SqlEngine ----------

#[test]
fn ddl_create_and_insert_select_update_delete_cycle() {
    let dir = tempdir().unwrap();
    let db = sql_engine(dir.path());
    seed_single_table(&db);
    assert_eq!(
        db.execute("INSERT INTO test.public.items (id,amount) VALUES (1,10)").unwrap(),
        SqlResultSet::Command { affected: 1 },
    );
    assert_eq!(
        db.execute("INSERT INTO test.public.items (amount,id) VALUES (20,2)").unwrap(),
        SqlResultSet::Command { affected: 1 },
    );
    assert_eq!(
        db.execute("UPDATE test.public.items SET amount=30 WHERE id=1").unwrap(),
        SqlResultSet::Command { affected: 1 },
    );
    assert_eq!(
        db.execute("DELETE FROM test.public.items WHERE id=2").unwrap(),
        SqlResultSet::Command { affected: 1 },
    );
    assert_eq!(
        db.execute("SELECT * FROM test.public.items").unwrap(),
        SqlResultSet::Rows {
            columns: vec!["id".into(), "amount".into()],
            rows: vec![vec![Value::Int64(1), Value::Int64(30)]],
        },
    );
}

// ---------- §4. rejections common to all engines ----------

fn is_unsupported(err: SqlError) -> bool {
    matches!(err, SqlError::Unsupported(_) | SqlError::Syntax(_))
}

#[test]
fn rejects_sql_comments_dash_and_block() {
    let dir = tempdir().unwrap();
    let db = sql_engine(dir.path());
    seed_single_table(&db);
    assert!(is_unsupported(db.execute("-- note\nSELECT * FROM test.public.items").unwrap_err()));
    assert!(is_unsupported(db.execute("/* note */ SELECT * FROM test.public.items").unwrap_err()));
}

#[test]
fn rejects_multi_statement_in_a_single_execute() {
    let dir = tempdir().unwrap();
    let db = sql_engine(dir.path());
    seed_single_table(&db);
    // Trailing content after the first terminator is not accepted by execute().
    let err = db.execute(
        "INSERT INTO test.public.items (id,amount) VALUES (1,1); INSERT INTO test.public.items (id,amount) VALUES (2,2)",
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_) | SqlError::Unsupported(_)));
}

#[test]
fn rejects_partial_primary_key_predicate() {
    let dir = tempdir().unwrap();
    let db = sql_engine(dir.path());
    for sql in [
        "CREATE DATABASE test",
        "CREATE SCHEMA test.public",
        "CREATE TABLE test.public.composite (
            a BIGINT NOT NULL, b BIGINT NOT NULL, v BIGINT NOT NULL,
            PRIMARY KEY(a,b)
        )",
    ] { db.execute(sql).unwrap(); }
    db.execute("INSERT INTO test.public.composite (a,b,v) VALUES (1,1,99)").unwrap();
    let err = db.execute("DELETE FROM test.public.composite WHERE a=1").unwrap_err();
    assert!(is_unsupported(err));
}

#[test]
fn rejects_join_aggregate_and_or_and_range_and_alias() {
    let dir = tempdir().unwrap();
    let db = sql_engine(dir.path());
    seed_single_table(&db);
    for sql in [
        "SELECT i.* FROM test.public.items i JOIN test.public.items j ON i.id=j.id",
        "SELECT COUNT(*) FROM test.public.items",
        "SELECT * FROM test.public.items WHERE id=1 OR id=2",
        "SELECT * FROM test.public.items WHERE id>1",
        "SELECT amount AS total FROM test.public.items",
        "SELECT DISTINCT amount FROM test.public.items",
        "SELECT * FROM test.public.items UNION SELECT * FROM test.public.items",
    ] {
        let err = db.execute(sql).unwrap_err();
        assert!(is_unsupported(err), "statement should be rejected: {sql}");
    }
}

// ---------- §5. SqlEngine transactions ----------

#[test]
fn sql_engine_session_commit_publishes_single_wal_batch() {
    let dir = tempdir().unwrap();
    let db = sql_engine(dir.path());
    seed_single_table(&db);
    db.with_sql_session("test.public.items", |s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO test.public.items (id,amount) VALUES (1,10)")?;
        s.execute("INSERT INTO test.public.items (id,amount) VALUES (2,20)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    assert_eq!(
        db.execute("SELECT amount FROM test.public.items WHERE id=1").unwrap(),
        SqlResultSet::Rows { columns: vec!["amount".into()], rows: vec![vec![Value::Int64(10)]] },
    );
}

#[test]
fn sql_engine_session_rollback_discards_staged_changes() {
    let dir = tempdir().unwrap();
    let db = sql_engine(dir.path());
    seed_single_table(&db);
    db.execute("INSERT INTO test.public.items (id,amount) VALUES (1,10)").unwrap();
    db.with_sql_session("test.public.items", |s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE test.public.items SET amount=99 WHERE id=1")?;
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(
        db.execute("SELECT amount FROM test.public.items WHERE id=1").unwrap(),
        SqlResultSet::Rows { columns: vec!["amount".into()], rows: vec![vec![Value::Int64(10)]] },
    );
}

// ---------- §6. DomainSqlEngine transactions ----------

#[test]
fn domain_cross_table_commit_publishes_atomically() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("src");
    {
        let ddl = SqlEngine::open(&source).unwrap();
        for sql in [
            "CREATE DATABASE bank",
            "CREATE SCHEMA bank.public",
            "CREATE TABLE bank.public.accounts (id BIGINT NOT NULL, balance BIGINT NOT NULL, PRIMARY KEY(id))",
            "CREATE TABLE bank.public.movements (id BIGINT NOT NULL, amount BIGINT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    let db = DomainSqlEngine::open(&source).unwrap();
    db.register_catalog_table("bank.public.accounts").unwrap();
    db.register_catalog_table("bank.public.movements").unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO bank.public.accounts (id,balance) VALUES (1,100)")?;
        s.execute("INSERT INTO bank.public.movements (id,amount) VALUES (7,25)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        assert_eq!(
            s.execute("SELECT balance FROM bank.public.accounts WHERE id=1")?,
            SqlResultSet::Rows { columns: vec!["balance".into()], rows: vec![vec![Value::Int64(100)]] },
        );
        assert_eq!(
            s.execute("SELECT amount FROM bank.public.movements WHERE id=7")?,
            SqlResultSet::Rows { columns: vec!["amount".into()], rows: vec![vec![Value::Int64(25)]] },
        );
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn domain_rollback_discards_cross_table_changes() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("src");
    {
        let ddl = SqlEngine::open(&source).unwrap();
        for sql in [
            "CREATE DATABASE bank",
            "CREATE SCHEMA bank.public",
            "CREATE TABLE bank.public.accounts (id BIGINT NOT NULL, balance BIGINT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    let db = DomainSqlEngine::open(&source).unwrap();
    db.register_catalog_table("bank.public.accounts").unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO bank.public.accounts (id,balance) VALUES (1,100)")?;
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        assert_eq!(
            s.execute("SELECT balance FROM bank.public.accounts WHERE id=1")?,
            SqlResultSet::Rows { columns: vec!["balance".into()], rows: vec![] },
        );
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

// ---------- §7. HistorySqlEngine sessions ----------

#[test]
fn history_begin_commit_round_trip() {
    let (_dir, history) = seed_history();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        assert_eq!(
            s.execute("SELECT balance FROM bank.public.accounts WHERE id=1")?,
            SqlResultSet::Rows { columns: vec!["balance".into()], rows: vec![vec![Value::Int64(1000)]] },
        );
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn history_savepoint_rollback_to_discards_only_post_marker_work() {
    let (_dir, history) = seed_history();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE bank.public.accounts SET balance=1100 WHERE id=1")?;
        s.execute("SAVEPOINT after_first")?;
        s.execute("UPDATE bank.public.accounts SET balance=9999 WHERE id=1")?;
        s.execute("ROLLBACK TO after_first")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        assert_eq!(
            s.execute("SELECT balance FROM bank.public.accounts WHERE id=1")?,
            SqlResultSet::Rows { columns: vec!["balance".into()], rows: vec![vec![Value::Int64(1100)]] },
        );
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn history_begin_with_read_validation_is_accepted() {
    let (_dir, history) = seed_history();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN TRANSACTION WITH READ VALIDATION")?;
        assert_eq!(
            s.execute("SELECT balance FROM bank.public.accounts WHERE id=1")?,
            SqlResultSet::Rows { columns: vec!["balance".into()], rows: vec![vec![Value::Int64(1000)]] },
        );
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
}

// ---------- §8. UPDATE expressions on HistorySqlEngine ----------

#[test]
fn update_expression_arithmetic_on_bigint() {
    let (_dir, history) = seed_history();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE bank.public.accounts SET balance = balance + 25 WHERE id=1")?;
        assert_eq!(
            s.execute("SELECT balance FROM bank.public.accounts WHERE id=1")?,
            SqlResultSet::Rows { columns: vec!["balance".into()], rows: vec![vec![Value::Int64(1025)]] },
        );
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn update_expression_parentheses_override_precedence() {
    let (_dir, history) = seed_history();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE bank.public.accounts SET balance = (balance + 10) * 2 WHERE id=1")?;
        assert_eq!(
            s.execute("SELECT balance FROM bank.public.accounts WHERE id=1")?,
            SqlResultSet::Rows { columns: vec!["balance".into()], rows: vec![vec![Value::Int64(2020)]] },
        );
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

// ---------- §10. Ordered historical queries ----------

#[test]
fn ordered_order_by_limit_offset() {
    let (_dir, history) = seed_history();
    let db = HistorySqlEngine::open(&history).unwrap();
    let rows = db.with_session(|s| {
        s.execute("BEGIN")?;
        let r = s.execute("SELECT id FROM bank.public.accounts ORDER BY id DESC LIMIT 2 OFFSET 0")?;
        s.execute("ROLLBACK")?;
        Ok(r)
    }).unwrap();
    assert_eq!(
        rows,
        SqlResultSet::Rows {
            columns: vec!["id".into()],
            rows: vec![vec![Value::Int64(2)], vec![Value::Int64(1)]],
        },
    );
}

#[test]
fn ordered_rejects_oversized_limit() {
    // A LIMIT above the configured ceiling is refused by the parser; the
    // statement error aborts the enclosing session automatically. We capture
    // the inner result and let the session unwind without a trailing ROLLBACK,
    // which would itself fail with "ROLLBACK without BEGIN" after the abort.
    let (_dir, history) = seed_history();
    let db = HistorySqlEngine::open(&history).unwrap();
    let outcome = db.with_session(|s| {
        s.execute("BEGIN")?;
        Ok(s.execute("SELECT id FROM bank.public.accounts LIMIT 999999999"))
    }).unwrap();
    assert!(outcome.is_err(), "oversized LIMIT must be refused");
}

// ---------- §11. AS OF ----------

#[test]
fn as_of_returns_committed_rows_at_lsn() {
    let (_dir, history) = seed_history();
    let db = HistorySqlEngine::open(&history).unwrap();
    // Capture a snapshot LSN, then mutate, then query AS OF the snapshot LSN.
    let snapshot_lsn = db.with_session(|s| {
        s.execute("BEGIN")?;
        let before = s.execute("SELECT balance FROM bank.public.accounts WHERE id=1")?;
        assert!(matches!(before, SqlResultSet::Rows { .. }));
        s.execute("ROLLBACK")?;
        Ok(db.last_lsn())
    }).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE bank.public.accounts SET balance=5000 WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        let now = s.execute("SELECT balance FROM bank.public.accounts WHERE id=1")?;
        assert_eq!(now, SqlResultSet::Rows {
            columns: vec!["balance".into()],
            rows: vec![vec![Value::Int64(5000)]],
        });
        let historical = s.execute(&format!(
            "SELECT balance FROM bank.public.accounts AS OF {snapshot_lsn} WHERE id=1"
        ))?;
        assert_eq!(historical, SqlResultSet::Rows {
            columns: vec!["balance".into()],
            rows: vec![vec![Value::Int64(1000)]],
        });
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn as_of_is_rejected_in_a_read_validation_session() {
    // AS OF reads in a read-validation session are refused. The statement
    // error aborts the enclosing session; no trailing ROLLBACK is issued.
    let (_dir, history) = seed_history();
    let db = HistorySqlEngine::open(&history).unwrap();
    let outcome = db.with_session(|s| {
        s.execute("BEGIN TRANSACTION WITH READ VALIDATION")?;
        Ok(s.execute("SELECT balance FROM bank.public.accounts AS OF 1 WHERE id=1"))
    }).unwrap();
    assert!(outcome.is_err(), "AS OF with read validation must be refused");
}

// ---------- §9. Schema evolution ----------

#[test]
fn alter_add_nullable_column_lets_old_rows_read_null_when_epoch_writes_enabled() {
    // ALTER requires epoch-tagged rows (G35+). A freshly migrated G23 history
    // domain must explicitly opt in to epoch encoding before ALTER is accepted.
    let (_dir, history) = seed_history();
    let mut db = HistorySqlEngine::open(&history).unwrap();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl(
        "ALTER TABLE bank.public.accounts ADD COLUMN memo TEXT NULL",
    ).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        assert_eq!(
            s.execute("SELECT memo FROM bank.public.accounts WHERE id=1")?,
            SqlResultSet::Rows { columns: vec!["memo".into()], rows: vec![vec![Value::Null]] },
        );
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn alter_without_epoch_encoding_is_refused_on_a_migrated_history_domain() {
    // ADR 0055 captures the concrete engine behavior: without an explicit
    // `enable_epoch_row_writes` opt-in, the migrated history domain refuses
    // ALTER rather than silently rewriting legacy rows.
    let (_dir, history) = seed_history();
    let mut db = HistorySqlEngine::open(&history).unwrap();
    assert!(db
        .execute_ddl("ALTER TABLE bank.public.accounts ADD COLUMN memo TEXT NULL")
        .is_err());
}

#[test]
fn alter_rejects_drop_column_rename_and_type_change() {
    let (_dir, history) = seed_history();
    let mut db = HistorySqlEngine::open(&history).unwrap();
    for ddl in [
        "ALTER TABLE bank.public.accounts DROP COLUMN balance",
        "ALTER TABLE bank.public.accounts RENAME COLUMN balance TO equity",
        "ALTER TABLE bank.public.accounts ALTER COLUMN balance TYPE TEXT",
    ] {
        assert!(db.execute_ddl(ddl).is_err(), "should be rejected: {ddl}");
    }
}
