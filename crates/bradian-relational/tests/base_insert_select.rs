//! ADR 0224 — INSERT INTO t (cols) SELECT cols FROM src.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.src (id BIGINT NOT NULL, name TEXT NOT NULL, qty BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.dst (id BIGINT NOT NULL, name TEXT NOT NULL, qty BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.src(id, name, qty) VALUES(1, 'a', 10), (2, 'b', 20), (3, 'c', 30)",
    ] { db.execute(sql).unwrap(); }
    db
}

fn affected(rs: SqlResultSet) -> u64 {
    match rs {
        SqlResultSet::Command { affected } => affected,
        other => panic!("expected Command; got {other:?}"),
    }
}

fn count(db: &SqlEngine, table: &str) -> i64 {
    match db.execute(&format!("SELECT COUNT(*) FROM {table}")).unwrap() {
        SqlResultSet::Rows { rows, .. } => match rows[0][0] {
            Value::Int64(n) => n,
            _ => panic!("expected count"),
        }
        _ => panic!("expected rows"),
    }
}

#[test]
fn insert_select_copies_all_rows() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let aff = affected(db.execute(
        "INSERT INTO t.public.dst(id, name, qty) SELECT id, name, qty FROM t.public.src"
    ).unwrap());
    assert_eq!(aff, 3);
    assert_eq!(count(&db, "t.public.dst"), 3);
}

#[test]
fn insert_select_with_pk_where() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let aff = affected(db.execute(
        "INSERT INTO t.public.dst(id, name, qty) SELECT id, name, qty FROM t.public.src WHERE id = 2"
    ).unwrap());
    assert_eq!(aff, 1);
}

#[test]
fn insert_select_column_count_mismatch_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "INSERT INTO t.public.dst(id, name, qty) SELECT id, name FROM t.public.src"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn insert_select_with_returning() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute(
        "INSERT INTO t.public.dst(id, name, qty) SELECT id, name, qty FROM t.public.src WHERE id = 1 RETURNING id"
    ).unwrap() {
        SqlResultSet::Rows { columns, rows, .. } => {
            assert_eq!(columns, vec!["id"]);
            assert_eq!(rows, vec![vec![Value::Int64(1)]]);
        }
        _ => panic!("expected rows"),
    }
}

#[test]
fn insert_select_from_empty_table() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.src (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.dst (id BIGINT NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    let aff = affected(db.execute(
        "INSERT INTO t.public.dst(id) SELECT id FROM t.public.src"
    ).unwrap());
    assert_eq!(aff, 0);
}

#[test]
fn insert_select_with_type_widen() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.src (id BIGINT NOT NULL, n INT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.dst (id BIGINT NOT NULL, n BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.src(id, n) VALUES(1, 42)",
    ] { db.execute(sql).unwrap(); }
    let aff = affected(db.execute(
        "INSERT INTO t.public.dst(id, n) SELECT id, n FROM t.public.src"
    ).unwrap());
    assert_eq!(aff, 1);
}

#[test]
fn insert_select_duplicate_pk_from_source_rejected_on_target() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.src (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.dst (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.src(id) VALUES(1)",
        "INSERT INTO t.public.dst(id) VALUES(1)",
    ] { db.execute(sql).unwrap(); }
    assert!(db.execute(
        "INSERT INTO t.public.dst(id) SELECT id FROM t.public.src"
    ).is_err());
}
