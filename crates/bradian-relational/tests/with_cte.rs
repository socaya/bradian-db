//! ADR 0274 — WITH / CTE (non-recursive, narrow scope).

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, note TEXT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, note) VALUES(1, 'a')",
        "INSERT INTO t.public.items(id, note) VALUES(2, 'b')",
        "INSERT INTO t.public.items(id, note) VALUES(3, 'c')",
    ] { db.execute(sql).unwrap(); }
    (dir, db)
}

fn ids(rs: SqlResultSet) -> Vec<i64> {
    match rs {
        SqlResultSet::Rows { rows, .. } =>
            rows.into_iter().map(|r| match r[0] {
                Value::Int64(v) => v,
                _ => panic!(),
            }).collect(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn simple_cte_select_star_returns_inner() {
    let (_dir, db) = seed();
    let got = ids(db.execute(
        "WITH x AS (SELECT id FROM t.public.items) SELECT * FROM x"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 3]);
}

#[test]
fn cte_with_outer_where_narrows_inner() {
    let (_dir, db) = seed();
    let got = ids(db.execute(
        "WITH x AS (SELECT * FROM t.public.items) SELECT id FROM x WHERE id = 2"
    ).unwrap());
    assert_eq!(got, vec![2]);
}

#[test]
fn cte_mismatched_outer_name_rejected() {
    let (_dir, db) = seed();
    let err = db.execute(
        "WITH x AS (SELECT id FROM t.public.items) SELECT id FROM y"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)), "got {err:?}");
}

#[test]
fn cte_with_projection_narrows_inner() {
    let (_dir, db) = seed();
    let got = ids(db.execute(
        "WITH x AS (SELECT * FROM t.public.items) SELECT id FROM x"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 3]);
}

#[test]
fn cte_missing_as_rejected() {
    let (_dir, db) = seed();
    let err = db.execute(
        "WITH x (SELECT id FROM t.public.items) SELECT * FROM x"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)), "got {err:?}");
}

#[test]
fn cte_unbalanced_parens_rejected() {
    let (_dir, db) = seed();
    let err = db.execute(
        "WITH x AS (SELECT id FROM t.public.items SELECT * FROM x"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)), "got {err:?}");
}

#[test]
fn cte_without_from_in_outer_rejected() {
    let (_dir, db) = seed();
    let err = db.execute(
        "WITH x AS (SELECT * FROM t.public.items) SELECT id"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)), "got {err:?}");
}
