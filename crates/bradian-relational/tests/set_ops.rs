//! ADR 0275 — INTERSECT / EXCEPT set operators.

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
        "INSERT INTO t.public.b(id) VALUES(2)",
        "INSERT INTO t.public.b(id) VALUES(3)",
        "INSERT INTO t.public.b(id) VALUES(4)",
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
fn intersect_keeps_common() {
    let (_dir, db) = seed();
    let got = ids(db.execute(
        "SELECT id FROM t.public.a INTERSECT SELECT id FROM t.public.b"
    ).unwrap());
    assert_eq!(got, vec![2, 3]);
}

#[test]
fn except_subtracts_right() {
    let (_dir, db) = seed();
    let got = ids(db.execute(
        "SELECT id FROM t.public.a EXCEPT SELECT id FROM t.public.b"
    ).unwrap());
    assert_eq!(got, vec![1]);
}

#[test]
fn except_dedups() {
    // EXCEPT returns DISTINCT.
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.a (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.b (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.a(id) VALUES(1)",
        "INSERT INTO t.public.a(id) VALUES(2)",
    ] { db.execute(sql).unwrap(); }
    let got = ids(db.execute(
        "SELECT id FROM t.public.a EXCEPT SELECT id FROM t.public.b"
    ).unwrap());
    assert_eq!(got, vec![1, 2]);
}

#[test]
fn intersect_with_union() {
    // Mixed chain: a INTERSECT b UNION c.
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.a (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.b (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.c (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.a(id) VALUES(1)",
        "INSERT INTO t.public.a(id) VALUES(2)",
        "INSERT INTO t.public.b(id) VALUES(2)",
        "INSERT INTO t.public.b(id) VALUES(3)",
        "INSERT INTO t.public.c(id) VALUES(5)",
    ] { db.execute(sql).unwrap(); }
    // (a INTERSECT b) = {2}; then UNION c = {2, 5}.
    let got = ids(db.execute(
        "SELECT id FROM t.public.a INTERSECT SELECT id FROM t.public.b UNION SELECT id FROM t.public.c"
    ).unwrap());
    assert_eq!(got, vec![2, 5]);
}

#[test]
fn intersect_empty_when_no_common() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.a (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.b (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.a(id) VALUES(1)",
        "INSERT INTO t.public.b(id) VALUES(99)",
    ] { db.execute(sql).unwrap(); }
    let got = ids(db.execute(
        "SELECT id FROM t.public.a INTERSECT SELECT id FROM t.public.b"
    ).unwrap());
    assert!(got.is_empty());
}
