//! ADR 0222 — TRUNCATE ... RETURNING.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, name) VALUES(1, 'alpha'), (2, 'bravo'), (3, 'charlie')",
    ] { db.execute(sql).unwrap(); }
    db
}

#[test]
fn truncate_without_returning_still_command() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("TRUNCATE TABLE t.public.items").unwrap() {
        SqlResultSet::Command { affected } => assert_eq!(affected, 3),
        other => panic!("expected Command; got {other:?}"),
    }
}

#[test]
fn truncate_returning_star() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("TRUNCATE TABLE t.public.items RETURNING *").unwrap() {
        SqlResultSet::Rows { columns, rows, .. } => {
            assert_eq!(columns, vec!["id", "name"]);
            assert_eq!(rows.len(), 3);
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn truncate_returning_subset() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("TRUNCATE TABLE t.public.items RETURNING id").unwrap() {
        SqlResultSet::Rows { columns, rows, .. } => {
            assert_eq!(columns, vec!["id"]);
            assert_eq!(rows.len(), 3);
        }
        other => panic!("expected Rows; got {other:?}"),
    }
    // Table is empty afterward.
    match db.execute("SELECT COUNT(*) FROM t.public.items").unwrap() {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Int64(0)),
        _ => panic!("count"),
    }
}

#[test]
fn truncate_returning_unknown_column_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute("TRUNCATE TABLE t.public.items RETURNING nope").unwrap_err();
    assert!(matches!(err, SqlError::Resolution(_)));
}

#[test]
fn truncate_empty_table_returns_empty_rows() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    match db.execute("TRUNCATE TABLE t.public.items RETURNING id").unwrap() {
        SqlResultSet::Rows { rows, .. } => assert!(rows.is_empty()),
        _ => panic!("expected empty rows"),
    }
}
