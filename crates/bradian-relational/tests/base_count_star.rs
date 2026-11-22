//! ADR 0213 — SELECT COUNT(*) across the base engines.

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
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.items(id, body) VALUES(1, '{"kind":"a"}')"#,
        r#"INSERT INTO t.public.items(id, body) VALUES(2, '{"kind":"a"}')"#,
        r#"INSERT INTO t.public.items(id, body) VALUES(3, '{"kind":"b"}')"#,
        r#"INSERT INTO t.public.items(id, body) VALUES(4, '{"kind":"b"}')"#,
        r#"INSERT INTO t.public.items(id, body) VALUES(5, '{"kind":"c"}')"#,
    ] { db.execute(sql).unwrap(); }
    db
}

fn single_count(rs: SqlResultSet) -> i64 {
    match rs {
        SqlResultSet::Rows { columns, rows } => {
            assert_eq!(columns, vec!["count"]);
            assert_eq!(rows.len(), 1);
            match rows[0][0] {
                Value::Int64(n) => n,
                ref other => panic!("expected Int64; got {other:?}"),
            }
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn count_star_on_empty_table() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    assert_eq!(
        single_count(db.execute("SELECT COUNT(*) FROM t.public.items").unwrap()),
        0,
    );
}

#[test]
fn count_star_full_scan() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        single_count(db.execute("SELECT COUNT(*) FROM t.public.items").unwrap()),
        5,
    );
}

#[test]
fn count_star_with_pk_where() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        single_count(db.execute("SELECT COUNT(*) FROM t.public.items WHERE id = 3").unwrap()),
        1,
    );
}

#[test]
fn count_star_with_pk_where_misses() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        single_count(db.execute("SELECT COUNT(*) FROM t.public.items WHERE id = 99").unwrap()),
        0,
    );
}

#[test]
fn count_star_with_json_path_where() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        single_count(db.execute(
            "SELECT COUNT(*) FROM t.public.items WHERE body ->> 'kind' = 'a'"
        ).unwrap()),
        2,
    );
}

#[test]
fn count_star_with_in_list() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        single_count(db.execute(
            "SELECT COUNT(*) FROM t.public.items WHERE body ->> 'kind' IN ('a','c')"
        ).unwrap()),
        3,
    );
}

#[test]
fn count_star_with_like() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        single_count(db.execute(
            "SELECT COUNT(*) FROM t.public.items WHERE body ->> 'kind' LIKE 'a%'"
        ).unwrap()),
        2,
    );
}

#[test]
fn count_without_star_is_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute("SELECT COUNT(id) FROM t.public.items").unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn count_star_before_other_projections_is_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // ADR 0213 scope: COUNT(*) owns the whole projection list.
    // A comma after it is a parse error (no mixing).
    let err = db.execute("SELECT COUNT(*), id FROM t.public.items").unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_) | SqlError::Unsupported(_)));
}

#[test]
fn count_star_ignores_order_by() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        single_count(db.execute(
            "SELECT COUNT(*) FROM t.public.items ORDER BY id"
        ).unwrap()),
        5,
    );
}

#[test]
fn domain_engine_count_star() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.items").unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.items(id) VALUES(1)")?;
        s.execute("INSERT INTO t.public.items(id) VALUES(2)")?;
        s.execute("INSERT INTO t.public.items(id) VALUES(3)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(0i64));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } =
            s.execute("SELECT COUNT(*) FROM t.public.items")?
        {
            if let Value::Int64(n) = rows[0][0] { *captured.lock().unwrap() = n; }
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), 3);
}

#[test]
fn mvcc_engine_count_star_with_where() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(dir.path()).unwrap();
        dom.register_catalog_table("t.public.items").unwrap();
    }
    let engine = MvccSqlEngine::open(dir.path()).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute(r#"INSERT INTO t.public.items(id, body) VALUES(1, '{"k":"x"}')"#)?;
        s.execute(r#"INSERT INTO t.public.items(id, body) VALUES(2, '{"k":"y"}')"#)?;
        s.execute(r#"INSERT INTO t.public.items(id, body) VALUES(3, '{"k":"x"}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(0i64));
    let captured = got.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } =
            s.execute("SELECT COUNT(*) FROM t.public.items WHERE body ->> 'k' = 'x'")?
        {
            if let Value::Int64(n) = rows[0][0] { *captured.lock().unwrap() = n; }
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), 2);
}
