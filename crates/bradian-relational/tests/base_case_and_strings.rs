//! ADR 0233 — CASE WHEN + CONCAT/SUBSTR/TRIM/REPLACE.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, status TEXT NOT NULL, score BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, name, status, score) VALUES \
         (1, '  Alpha  ', 'open', 50), \
         (2, 'Bravo', 'closed', 90), \
         (3, 'Charlie', 'open', 25)",
    ] { db.execute(sql).unwrap(); }
    db
}

fn rows_of(rs: SqlResultSet) -> (Vec<String>, Vec<Vec<Value>>) {
    match rs {
        SqlResultSet::Rows { columns, rows } => (columns, rows),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn case_classifies_score() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (cols, rows) = rows_of(db.execute(
        "SELECT id, CASE WHEN score >= 70 THEN 'high' WHEN score >= 40 THEN 'mid' ELSE 'low' END FROM t.public.items"
    ).unwrap());
    assert_eq!(cols, vec!["id", "case"]);
    assert_eq!(rows.len(), 3);
    // Row 1: score=50 → mid; row 2: score=90 → high; row 3: 25 → low.
    assert_eq!(rows[0][1], Value::String("mid".into()));
    assert_eq!(rows[1][1], Value::String("high".into()));
    assert_eq!(rows[2][1], Value::String("low".into()));
}

#[test]
fn case_without_else_returns_null() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT CASE WHEN score > 100 THEN 'way-high' END FROM t.public.items WHERE id = 1"
    ).unwrap());
    assert_eq!(rows[0][0], Value::Null);
}

#[test]
fn case_with_text_column() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT CASE WHEN status = 'open' THEN 'active' ELSE 'done' END FROM t.public.items"
    ).unwrap());
    assert_eq!(rows[0][0], Value::String("active".into()));
    assert_eq!(rows[1][0], Value::String("done".into()));
    assert_eq!(rows[2][0], Value::String("active".into()));
}

#[test]
fn concat_columns_and_literals() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT CONCAT(name, '-', status) FROM t.public.items WHERE id = 2"
    ).unwrap());
    // Note row 2 name is 'Bravo'.
    assert_eq!(rows, vec![vec![Value::String("Bravo-closed".into())]]);
}

#[test]
fn concat_propagates_null() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, a TEXT, b TEXT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, a, b) VALUES(1, 'x', NULL)",
    ] { db.execute(sql).unwrap(); }
    let (_, rows) = rows_of(db.execute(
        "SELECT CONCAT(a, b) FROM t.public.items WHERE id = 1"
    ).unwrap());
    assert_eq!(rows[0][0], Value::Null);
}

#[test]
fn substr_two_arg() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // SUBSTR('Charlie', 3) → 'arlie' (1-based start).
    let (_, rows) = rows_of(db.execute(
        "SELECT SUBSTR(name, 3) FROM t.public.items WHERE id = 3"
    ).unwrap());
    assert_eq!(rows, vec![vec![Value::String("arlie".into())]]);
}

#[test]
fn substr_three_arg() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // SUBSTR('Bravo', 2, 3) → 'rav'.
    let (_, rows) = rows_of(db.execute(
        "SELECT SUBSTR(name, 2, 3) FROM t.public.items WHERE id = 2"
    ).unwrap());
    assert_eq!(rows, vec![vec![Value::String("rav".into())]]);
}

#[test]
fn substr_clamps_out_of_range() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // SUBSTR('Bravo', 10) → ''.
    let (_, rows) = rows_of(db.execute(
        "SELECT SUBSTR(name, 10) FROM t.public.items WHERE id = 2"
    ).unwrap());
    assert_eq!(rows, vec![vec![Value::String("".into())]]);
}

#[test]
fn trim_strips_whitespace() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT TRIM(name) FROM t.public.items WHERE id = 1"
    ).unwrap());
    assert_eq!(rows, vec![vec![Value::String("Alpha".into())]]);
}

#[test]
fn replace_substitutes_substring() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT REPLACE(name, 'a', 'Z') FROM t.public.items WHERE id = 2"
    ).unwrap());
    assert_eq!(rows, vec![vec![Value::String("BrZvo".into())]]);
}

#[test]
fn replace_empty_from_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT REPLACE(name, '', 'x') FROM t.public.items WHERE id = 1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

#[test]
fn case_combines_with_coalesce() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, label TEXT, score BIGINT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, label, score) VALUES(1, NULL, 80), (2, 'x', NULL)",
    ] { db.execute(sql).unwrap(); }
    // id 1: label NULL → fall through to CASE score=80 → 'high'.
    // Our grammar doesn't allow nested FuncExpr in function args,
    // so split the two tests.
    let (_, rows) = rows_of(db.execute(
        "SELECT COALESCE(label, 'blank') FROM t.public.items WHERE id = 1"
    ).unwrap());
    assert_eq!(rows[0][0], Value::String("blank".into()));
    let (_, rows) = rows_of(db.execute(
        "SELECT CASE WHEN score >= 70 THEN 'high' ELSE 'low' END FROM t.public.items WHERE id = 1"
    ).unwrap());
    assert_eq!(rows[0][0], Value::String("high".into()));
}
