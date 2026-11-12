//! Native Bradian SQL profile: file-backed DDL, DML, restart and strict errors.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn command(engine: &SqlEngine, sql: &str, expected: u64) {
    assert_eq!(
        engine.execute(sql).unwrap(),
        SqlResultSet::Command { affected: expected },
        "{sql}",
    );
}

fn setup(engine: &SqlEngine) {
    command(engine, "CREATE DATABASE billing;", 0);
    command(engine, "CREATE SCHEMA billing.public;", 0);
    command(engine,
        "CREATE TABLE billing.public.invoices (id BIGINT NOT NULL, customer VARCHAR(50) NOT NULL, amount DECIMAL(12,2) NOT NULL, settled BOOLEAN NOT NULL, note TEXT NULL, PRIMARY KEY(id));",
        0);
}

#[test]
fn native_sql_create_insert_select_update_delete_and_restart() {
    let base = tempdir().unwrap();
    {
        let db = SqlEngine::open(base.path()).unwrap();
        setup(&db);
        command(&db, "INSERT INTO billing.public.invoices (id,customer,amount,settled,note) VALUES (7,'Alice',1200.75,false,NULL);", 1);
        command(&db, "INSERT INTO billing.public.invoices (id,customer,amount,settled,note) VALUES (8,'Bob',99.00,true,'first order');", 1);
        assert_eq!(db.execute("SELECT customer,amount FROM billing.public.invoices WHERE id = 7;").unwrap(),
            SqlResultSet::Rows {
                columns: vec!["customer".into(), "amount".into()],
                rows: vec![vec![Value::String("Alice".into()), Value::Decimal(120075)]],
            });
        command(&db, "UPDATE billing.public.invoices SET settled = true, note = 'paid' WHERE id=7;", 1);
        command(&db, "DELETE FROM billing.public.invoices WHERE id=8;", 1);
        assert_eq!(db.execute("SELECT * FROM billing.public.invoices;").unwrap(),
            SqlResultSet::Rows {
                columns: vec!["id".into(), "customer".into(), "amount".into(), "settled".into(), "note".into()],
                rows: vec![vec![
                    Value::Int64(7), Value::String("Alice".into()), Value::Decimal(120075),
                    Value::Boolean(true), Value::String("paid".into()),
                ]],
            });
    }
    let reopened = SqlEngine::open(base.path()).unwrap();
    assert_eq!(reopened.execute("SELECT id, note FROM billing.public.invoices WHERE id=7").unwrap(),
        SqlResultSet::Rows {
            columns: vec!["id".into(), "note".into()],
            rows: vec![vec![Value::Int64(7), Value::String("paid".into())]],
        });
    assert_eq!(reopened.execute("SELECT id FROM billing.public.invoices WHERE id=8").unwrap(),
        SqlResultSet::Rows { columns: vec!["id".into()], rows: vec![] });
    command(&reopened, "INSERT INTO billing.public.invoices (id,customer,amount,settled,note) VALUES (9,'Eve',0.00,false,NULL)", 1);
}

#[test]
fn rejected_sql_never_mutates_a_preexisting_row() {
    let base = tempdir().unwrap();
    let db = SqlEngine::open(base.path()).unwrap();
    setup(&db);
    command(&db, "INSERT INTO billing.public.invoices(id,customer,amount,settled,note) VALUES(1,'One',10.00,false,NULL)", 1);
    let wrong = [
        "INSERT INTO billing.public.invoices (id,customer,amount,settled,note) VALUES (1,'Duplicate',2.00,false,NULL)",
        "INSERT INTO billing.public.invoices (id,customer,amount,settled,note) VALUES (2,'Missing',2.001,false,NULL)",
        "INSERT INTO billing.public.invoices (id,customer,amount,settled,note) VALUES (3,'Longer than fifty chars 1234567890123456789012345678901234567890',2.00,false,NULL)",
        "INSERT INTO billing.public.invoices(id,id,amount,settled,note) VALUES (2,2,3.00,false,NULL)",
        "UPDATE billing.public.invoices SET id = 2 WHERE id = 1",
        "DELETE FROM billing.public.invoices",
        "UPDATE billing.public.invoices SET settled = false WHERE customer='One'",
        "SELECT id FROM billing.public.invoices WHERE settled = true",
        "SELECT id FROM billing.public.invoices; DELETE FROM billing.public.invoices WHERE id=1;",
        "SELECT no_such_column FROM billing.public.invoices",
        "CREATE TABLE billing.public.invalid (id BIGINT NOT NULL)",
        "SELECT * FROM billing.public.invoices WHERE id = 1 OR id=2",
    ];
    for sql in wrong {
        assert!(db.execute(sql).is_err(), "unsafe statement accepted: {sql}");
    }
    assert_eq!(db.execute("SELECT id,customer FROM billing.public.invoices WHERE id=1").unwrap(),
        SqlResultSet::Rows {
            columns: vec!["id".into(),"customer".into()],
            rows: vec![vec![Value::Int64(1),Value::String("One".into())]],
        });
}

#[test]
fn sql_literals_handle_escaped_quotes_nulls_boolean_and_negative_values() {
    let base = tempdir().unwrap();
    let db = SqlEngine::open(base.path()).unwrap();
    setup(&db);
    command(&db, "INSERT INTO billing.public.invoices(id,customer,amount,settled,note) VALUES (-2,'O''Reilly',-12.50,true,NULL);", 1);
    assert_eq!(db.execute("SELECT customer,amount FROM billing.public.invoices WHERE id=-2;").unwrap(),
        SqlResultSet::Rows {
            columns: vec!["customer".into(),"amount".into()],
            rows: vec![vec![Value::String("O'Reilly".into()),Value::Decimal(-1250)]],
        });
    assert!(matches!(
        db.execute("INSERT INTO billing.public.invoices(id,customer,amount,settled,note) VALUES (9,'x',5.00,NULL,NULL)"),
        Err(SqlError::Type(_)),
    ));
    assert!(matches!(db.execute("INSERT INTO billing.public.invoices(id,customer,amount,settled,note) VALUES (9,'unterminated,5.00,false,NULL)"),
        Err(SqlError::Syntax(_))));
}

#[test]
fn fully_qualified_names_are_enforced_and_unknown_objects_fail() {
    let base = tempdir().unwrap();
    let db = SqlEngine::open(base.path()).unwrap();
    assert!(matches!(db.execute("CREATE SCHEMA nonexistent.public;"), Err(SqlError::Resolution(_))));
    setup(&db);
    for sql in [
        "SELECT * FROM invoices;",
        "INSERT INTO billing.public.missing(id) VALUES(1);",
        "CREATE SCHEMA public;",
        "CREATE TABLE invoices (id BIGINT NOT NULL, PRIMARY KEY(id));",
        "DROP TABLE billing.public.invoices;",
        "BEGIN;",
        "SELECT * FROM billing.public.invoices ORDER BY id",
    ] {
        assert!(db.execute(sql).is_err(), "unsupported query accepted: {sql}");
    }
}

#[test]
fn composite_primary_key_predicate_uses_declared_key_order() {
    let base = tempdir().unwrap();
    let db = SqlEngine::open(base.path()).unwrap();
    command(&db,"CREATE DATABASE operational",0);
    command(&db,"CREATE SCHEMA operational.public",0);
    command(&db,"CREATE TABLE operational.public.keys (tenant VARCHAR(20) NOT NULL, code VARCHAR(20) NOT NULL, payload TEXT NOT NULL, PRIMARY KEY(tenant,code))",0);
    command(&db,"INSERT INTO operational.public.keys(tenant,code,payload) VALUES ('a','bc','first')",1);
    command(&db,"INSERT INTO operational.public.keys(tenant,code,payload) VALUES ('ab','c','second')",1);
    assert_eq!(db.execute("SELECT payload FROM operational.public.keys WHERE code='bc' AND tenant='a'").unwrap(),
        SqlResultSet::Rows {columns:vec!["payload".into()],rows:vec![vec![Value::String("first".into())]]});
    command(&db,"UPDATE operational.public.keys SET payload='updated' WHERE code='c' AND tenant='ab'",1);
    assert_eq!(db.execute("SELECT payload FROM operational.public.keys WHERE tenant='ab' AND code='c'").unwrap(),
        SqlResultSet::Rows {columns:vec!["payload".into()],rows:vec![vec![Value::String("updated".into())]]});
    command(&db,"DELETE FROM operational.public.keys WHERE code='bc' AND tenant='a'",1);
    assert_eq!(db.execute("SELECT * FROM operational.public.keys").unwrap(),
        SqlResultSet::Rows {columns:vec!["tenant".into(),"code".into(),"payload".into()],rows:vec![vec![Value::String("ab".into()),Value::String("c".into()),Value::String("updated".into())]]});
}

#[test]
fn sql_insert_column_order_can_differ_from_schema_order() {
    let base = tempdir().unwrap();
    let db = SqlEngine::open(base.path()).unwrap();
    setup(&db);
    command(&db,"INSERT INTO billing.public.invoices(note,settled,amount,customer,id) VALUES (NULL,false,2.35,'Reverse',42)",1);
    assert_eq!(db.execute("SELECT id,customer,amount FROM billing.public.invoices WHERE id=42").unwrap(),
        SqlResultSet::Rows {columns:vec!["id".into(),"customer".into(),"amount".into()],rows:vec![vec![Value::Int64(42),Value::String("Reverse".into()),Value::Decimal(235)]]});
}


#[test]
fn typed_sql_table_transaction_commits_multiple_rows_as_one_wal_record() {
    let root = tempdir().unwrap();
    {
        let engine = SqlEngine::open(root.path()).unwrap();
        setup(&engine);
        let (count, lsn) = engine.transact_table("billing.public.invoices", |tx| {
            let before = tx.start_lsn();
            tx.insert(&[
                Value::Int64(11), Value::String("Alice".into()),
                Value::Decimal(10000), Value::Boolean(false), Value::Null,
            ])?;
            tx.insert(&[
                Value::Int64(12), Value::String("Bob".into()),
                Value::Decimal(20000), Value::Boolean(false), Value::Null,
            ])?;
            assert_eq!(tx.scan().len(), 2);
            Ok((before, tx.scan().len()))
        }).unwrap();
        assert_eq!(count.1, 2);
        assert_eq!(lsn, count.0 + 1);
        assert_eq!(engine.execute(
            "SELECT id,customer FROM billing.public.invoices"
        ).unwrap(), SqlResultSet::Rows {
            columns: vec!["id".into(), "customer".into()],
            rows: vec![
                vec![Value::Int64(11), Value::String("Alice".into())],
                vec![Value::Int64(12), Value::String("Bob".into())],
            ],
        });
    }
    let reopened = SqlEngine::open(root.path()).unwrap();
    assert_eq!(reopened.execute(
        "SELECT customer FROM billing.public.invoices WHERE id=12"
    ).unwrap(), SqlResultSet::Rows {
        columns: vec!["customer".into()],
        rows: vec![vec![Value::String("Bob".into())]],
    });
}

#[test]
fn typed_sql_transaction_closure_error_aborts_all_changes() {
    let root = tempdir().unwrap();
    let engine = SqlEngine::open(root.path()).unwrap();
    setup(&engine);
    let result: Result<((), u64), SqlError> =
        engine.transact_table("billing.public.invoices", |tx| {
            tx.insert(&[
                Value::Int64(30), Value::String("first".into()),
                Value::Decimal(100), Value::Boolean(false), Value::Null,
            ])?;
            Err(SqlError::Unsupported("cancel requested".into()))
        });
    assert!(matches!(result, Err(SqlError::Unsupported(_))));
    assert_eq!(engine.execute(
        "SELECT id FROM billing.public.invoices WHERE id=30"
    ).unwrap(), SqlResultSet::Rows { columns: vec!["id".into()], rows: vec![] });
}

#[test]
fn transaction_api_refuses_unqualified_and_unknown_tables() {
    let root = tempdir().unwrap();
    let engine = SqlEngine::open(root.path()).unwrap();
    setup(&engine);
    let callback = |_tx: &mut bradian_relational::typed_table::TableTransaction<'_>| {
        Ok::<_, SqlError>(())
    };
    assert!(engine.transact_table("invoices", callback).is_err());
    assert!(engine.transact_table("billing.public.absent", |_| Ok(())).is_err());
}
