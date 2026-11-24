//! ADR 0271 — EXISTS (SELECT …) subquery in WHERE.

use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.a (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.b (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.a(id) VALUES(1)",
        "INSERT INTO t.public.a(id) VALUES(2)",
        "INSERT INTO t.public.a(id) VALUES(3)",
    ] { db.execute(sql).unwrap(); }
    (dir, db)
}

fn ids(rs: SqlResultSet) -> Vec<i64> {
    match rs {
        SqlResultSet::Rows { rows, .. } =>
            rows.into_iter().map(|r| match r[0] { Value::Int64(v) => v, _ => panic!() }).collect(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn exists_returns_all_when_inner_non_empty() {
    let (_dir, db) = seed();
    db.execute("INSERT INTO t.public.b(id) VALUES(100)").unwrap();
    let got = ids(db.execute(
        "SELECT id FROM t.public.a WHERE EXISTS (SELECT id FROM t.public.b)"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 3]);
}

#[test]
fn exists_returns_none_when_inner_empty() {
    let (_dir, db) = seed();
    let got = ids(db.execute(
        "SELECT id FROM t.public.a WHERE EXISTS (SELECT id FROM t.public.b)"
    ).unwrap());
    assert!(got.is_empty());
}

#[test]
fn where_true_matches_everything() {
    let (_dir, db) = seed();
    let got = ids(db.execute("SELECT id FROM t.public.a WHERE TRUE").unwrap());
    assert_eq!(got, vec![1, 2, 3]);
}

#[test]
fn where_false_matches_nothing() {
    let (_dir, db) = seed();
    let got = ids(db.execute("SELECT id FROM t.public.a WHERE FALSE").unwrap());
    assert!(got.is_empty());
}

#[test]
fn exists_in_and_chain() {
    let (_dir, db) = seed();
    db.execute("INSERT INTO t.public.b(id) VALUES(100)").unwrap();
    // EXISTS TRUE AND id = 2 → row 2
    let got = ids(db.execute(
        "SELECT id FROM t.public.a WHERE EXISTS (SELECT id FROM t.public.b) AND id = 2"
    ).unwrap());
    assert_eq!(got, vec![2]);
}

#[test]
fn exists_in_or_chain() {
    let (_dir, db) = seed();
    // EXISTS is FALSE but id=1 still matches.
    let got = ids(db.execute(
        "SELECT id FROM t.public.a WHERE EXISTS (SELECT id FROM t.public.b) OR id = 1"
    ).unwrap());
    assert_eq!(got, vec![1]);
}

#[test]
fn exists_under_not() {
    let (_dir, db) = seed();
    // Inner is empty → EXISTS false → NOT EXISTS true → all rows.
    let got = ids(db.execute(
        "SELECT id FROM t.public.a WHERE NOT EXISTS (SELECT id FROM t.public.b)"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 3]);
}
