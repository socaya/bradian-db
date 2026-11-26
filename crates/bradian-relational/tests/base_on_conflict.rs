//! ADR 0223 — INSERT ... ON CONFLICT DO NOTHING / DO UPDATE.

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::mvcc_sql::MvccSqlEngine;
use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, qty BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, name, qty) VALUES(1, 'alpha', 10)",
    ] { db.execute(sql).unwrap(); }
    db
}

fn affected(rs: SqlResultSet) -> u64 {
    match rs {
        SqlResultSet::Command { affected } => affected,
        other => panic!("expected Command; got {other:?}"),
    }
}

fn row_of(db: &SqlEngine, id: i64) -> Vec<Value> {
    match db.execute(&format!("SELECT * FROM t.public.items WHERE id = {id}")).unwrap() {
        SqlResultSet::Rows { rows, .. } => rows[0].clone(),
        _ => panic!("expected row"),
    }
}

#[test]
fn on_conflict_do_nothing_skips_existing() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // id=1 exists → skip.
    let aff = affected(db.execute(
        "INSERT INTO t.public.items(id, name, qty) VALUES(1, 'collision', 99) ON CONFLICT (id) DO NOTHING"
    ).unwrap());
    assert_eq!(aff, 0);
    // Existing row unchanged.
    assert_eq!(row_of(&db, 1)[1], Value::String("alpha".into()));
}

#[test]
fn on_conflict_do_nothing_allows_new_row() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let aff = affected(db.execute(
        "INSERT INTO t.public.items(id, name, qty) VALUES(2, 'bravo', 20) ON CONFLICT (id) DO NOTHING"
    ).unwrap());
    assert_eq!(aff, 1);
    assert_eq!(row_of(&db, 2)[1], Value::String("bravo".into()));
}

#[test]
fn on_conflict_do_update_mutates_existing() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let aff = affected(db.execute(
        "INSERT INTO t.public.items(id, name, qty) VALUES(1, 'new', 100) ON CONFLICT (id) DO UPDATE SET name = 'upserted', qty = 42"
    ).unwrap());
    assert_eq!(aff, 1);
    let row = row_of(&db, 1);
    assert_eq!(row[1], Value::String("upserted".into()));
    assert_eq!(row[2], Value::Int64(42));
}

#[test]
fn on_conflict_do_update_mixed_hit_and_miss() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // id=1 exists (update), id=2 new (insert).
    let aff = affected(db.execute(
        "INSERT INTO t.public.items(id, name, qty) VALUES(1, 'x', 11), (2, 'y', 22) ON CONFLICT (id) DO UPDATE SET qty = 99"
    ).unwrap());
    assert_eq!(aff, 2);
    assert_eq!(row_of(&db, 1)[2], Value::Int64(99));
    assert_eq!(row_of(&db, 2)[1], Value::String("y".into()));
}

#[test]
fn on_conflict_returning_captures_both_shapes() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let rs = db.execute(
        "INSERT INTO t.public.items(id, name, qty) VALUES(1, 'x', 11), (2, 'y', 22) ON CONFLICT (id) DO UPDATE SET qty = 99 RETURNING id, qty"
    ).unwrap();
    match rs {
        SqlResultSet::Rows { columns, rows, .. } => {
            assert_eq!(columns, vec!["id", "qty"]);
            assert_eq!(rows[0], vec![Value::Int64(1), Value::Int64(99)]);
            assert_eq!(rows[1], vec![Value::Int64(2), Value::Int64(22)]);
        }
        _ => panic!("expected rows"),
    }
}

#[test]
fn on_conflict_pk_update_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "INSERT INTO t.public.items(id, name, qty) VALUES(1, 'x', 5) ON CONFLICT (id) DO UPDATE SET id = 99"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn on_conflict_target_not_pk_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "INSERT INTO t.public.items(id, name, qty) VALUES(1, 'x', 5) ON CONFLICT (name) DO NOTHING"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn domain_engine_on_conflict_update() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.items").unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.items(id, name) VALUES(1, 'first')")?;
        s.execute("INSERT INTO t.public.items(id, name) VALUES(1, 'collision') ON CONFLICT (id) DO UPDATE SET name = 'updated'")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got: std::sync::Arc<std::sync::Mutex<Vec<Vec<Value>>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } =
            s.execute("SELECT * FROM t.public.items")?
        {
            *captured.lock().unwrap() = rows;
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(got.lock().unwrap()[0][1], Value::String("updated".into()));
}

#[test]
fn mvcc_engine_on_conflict_do_nothing() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(dir.path()).unwrap();
        dom.register_catalog_table("t.public.items").unwrap();
    }
    let engine = MvccSqlEngine::open(dir.path()).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.items(id) VALUES(1)")?;
        s.execute("INSERT INTO t.public.items(id) VALUES(1) ON CONFLICT (id) DO NOTHING")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got: std::sync::Arc<std::sync::Mutex<i64>> = std::sync::Arc::new(std::sync::Mutex::new(0));
    let captured = got.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } = s.execute("SELECT COUNT(*) FROM t.public.items")? {
            if let Value::Int64(n) = rows[0][0] { *captured.lock().unwrap() = n; }
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), 1);
}
