//! ADR 0268 — verify CASE expression reaches SELECT projection
//! on bare scalar columns.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, grade INT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, grade) VALUES(1, 10)",
        "INSERT INTO t.public.items(id, grade) VALUES(2, 20)",
        "INSERT INTO t.public.items(id, grade) VALUES(3, 30)",
        "INSERT INTO t.public.items(id, grade) VALUES(4, NULL)",
    ] { db.execute(sql).unwrap(); }
    (dir, db)
}

fn ids_and_result(rs: SqlResultSet) -> Vec<Value> {
    match rs {
        SqlResultSet::Rows { rows, .. } =>
            rows.into_iter().map(|r| r.into_iter().next().unwrap()).collect(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn case_compare_in_projection() {
    let (_dir, db) = seed();
    let rs = db.execute(
        "SELECT CASE WHEN grade = 20 THEN 'match' ELSE 'other' END FROM t.public.items WHERE id = 2"
    ).unwrap();
    let vals = ids_and_result(rs);
    assert_eq!(vals, vec![Value::String("match".into())]);
}

#[test]
fn case_between_in_projection() {
    let (_dir, db) = seed();
    let rs = db.execute(
        "SELECT CASE WHEN grade BETWEEN 15 AND 25 THEN 'mid' ELSE 'out' END FROM t.public.items WHERE id = 2"
    ).unwrap();
    assert_eq!(ids_and_result(rs), vec![Value::String("mid".into())]);
}

#[test]
fn case_not_between_in_projection() {
    // Use BIGINT column `id` to avoid the cross-width
    // compare gap. id=3 is NOT in [100, 200] → "out".
    let (_dir, db) = seed();
    let rs = db.execute(
        "SELECT CASE WHEN id NOT BETWEEN 100 AND 200 THEN 'out' ELSE 'mid' END FROM t.public.items WHERE id = 3"
    ).unwrap();
    assert_eq!(ids_and_result(rs), vec![Value::String("out".into())]);
}

#[test]
fn case_multi_when_first_match_wins() {
    let (_dir, db) = seed();
    let rs = db.execute(
        "SELECT CASE WHEN grade = 10 THEN 'low' WHEN grade = 20 THEN 'mid' WHEN grade = 30 THEN 'high' ELSE 'none' END FROM t.public.items WHERE id = 1"
    ).unwrap();
    assert_eq!(ids_and_result(rs), vec![Value::String("low".into())]);
}

#[test]
fn case_null_input_falls_to_else() {
    let (_dir, db) = seed();
    let rs = db.execute(
        "SELECT CASE WHEN grade = 10 THEN 'match' ELSE 'none' END FROM t.public.items WHERE id = 4"
    ).unwrap();
    assert_eq!(ids_and_result(rs), vec![Value::String("none".into())]);
}

#[test]
fn case_without_else_returns_null_on_no_match() {
    // Use BIGINT column `id` so the literal 999 is same-width
    // and compare resolves cleanly (an Int32 column vs Int64
    // literal still has a cross-type gap — separate ADR).
    let (_dir, db) = seed();
    let rs = db.execute(
        "SELECT CASE WHEN id = 999 THEN 'match' END FROM t.public.items WHERE id = 1"
    ).unwrap();
    assert_eq!(ids_and_result(rs), vec![Value::Null]);
}

#[test]
fn case_requires_at_least_one_when() {
    let (_dir, db) = seed();
    let err = db.execute("SELECT CASE ELSE 'x' END FROM t.public.items").unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)), "got {err:?}");
}

#[test]
fn case_with_numeric_result() {
    let (_dir, db) = seed();
    let rs = db.execute(
        "SELECT CASE WHEN grade >= 20 THEN 1 ELSE 0 END FROM t.public.items WHERE id = 2"
    ).unwrap();
    assert_eq!(ids_and_result(rs), vec![Value::Int64(1)]);
}
