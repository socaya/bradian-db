//! ADR 0239 — arithmetic infix in expressions.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, qty BIGINT NOT NULL, price BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, qty, price) VALUES(1, 3, 10), (2, 5, 20), (3, -2, 7)",
    ] { db.execute(sql).unwrap(); }
    db
}

fn rows_of(rs: SqlResultSet) -> Vec<Vec<Value>> {
    match rs {
        SqlResultSet::Rows { rows, .. } => rows,
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn addition_and_subtraction() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = rows_of(db.execute("SELECT qty + 1, qty - 1 FROM t.public.items WHERE id = 1").unwrap());
    assert_eq!(got, vec![vec![Value::Int64(4), Value::Int64(2)]]);
}

#[test]
fn multiplication() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = rows_of(db.execute("SELECT qty * price FROM t.public.items WHERE id = 1").unwrap());
    assert_eq!(got, vec![vec![Value::Int64(30)]]);
}

#[test]
fn division_truncates_toward_zero() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = rows_of(db.execute("SELECT price / qty FROM t.public.items WHERE id = 1").unwrap());
    // 10 / 3 = 3 (integer truncation).
    assert_eq!(got, vec![vec![Value::Int64(3)]]);
}

#[test]
fn modulo() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = rows_of(db.execute("SELECT price % qty FROM t.public.items WHERE id = 1").unwrap());
    assert_eq!(got, vec![vec![Value::Int64(1)]]);
}

#[test]
fn division_by_zero_rejects() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, a BIGINT NOT NULL, b BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, a, b) VALUES(1, 10, 0)",
    ] { db.execute(sql).unwrap(); }
    let err = db.execute("SELECT a / b FROM t.public.items").unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

#[test]
fn precedence_multiplies_before_add() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // 3 + 5 * 10 = 3 + 50 = 53. If left-to-right it would be 80.
    let got = rows_of(db.execute("SELECT qty + 5 * price FROM t.public.items WHERE id = 1").unwrap());
    assert_eq!(got, vec![vec![Value::Int64(53)]]);
}

#[test]
fn null_propagates_through_arithmetic() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, a BIGINT, b BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, a, b) VALUES(1, NULL, 5)",
    ] { db.execute(sql).unwrap(); }
    let got = rows_of(db.execute("SELECT a + b FROM t.public.items").unwrap());
    assert_eq!(got, vec![vec![Value::Null]]);
}

#[test]
fn arithmetic_in_case_predicate() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // qty + 5 > 7 for ids 1,2 (8 > 7 and 10 > 7); id 3's qty=-2 so 3 ≯ 7 → 'low'.
    let got = rows_of(db.execute(
        "SELECT CASE WHEN qty + 5 > 7 THEN 'high' ELSE 'low' END FROM t.public.items"
    ).unwrap());
    assert_eq!(got[0][0], Value::String("high".into()));
    assert_eq!(got[1][0], Value::String("high".into()));
    assert_eq!(got[2][0], Value::String("low".into()));
}

#[test]
fn arithmetic_inside_aggregate() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // SUM(qty * price) = 3*10 + 5*20 + (-2)*7 = 30 + 100 - 14 = 116.
    let got = rows_of(db.execute("SELECT SUM(qty * price) FROM t.public.items").unwrap());
    assert_eq!(got, vec![vec![Value::Int64(116)]]);
}

#[test]
fn negative_literal_still_parses() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = rows_of(db.execute("SELECT qty + -1 FROM t.public.items WHERE id = 1").unwrap());
    assert_eq!(got, vec![vec![Value::Int64(2)]]);
}

#[test]
fn non_numeric_arithmetic_rejected() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, name) VALUES(1, 'abc')",
    ] { db.execute(sql).unwrap(); }
    let err = db.execute("SELECT name + 1 FROM t.public.items").unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}
