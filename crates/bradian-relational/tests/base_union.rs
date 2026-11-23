//! ADR 0225 — UNION / UNION ALL.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.a (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.b (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.a(id, name) VALUES(1, 'alpha'), (2, 'bravo')",
        "INSERT INTO t.public.b(id, name) VALUES(2, 'bravo'), (3, 'charlie')",
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
fn union_dedups_duplicates() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (cols, rows) = rows_of(db.execute(
        "SELECT id, name FROM t.public.a UNION SELECT id, name FROM t.public.b"
    ).unwrap());
    assert_eq!(cols, vec!["id", "name"]);
    // Union dedups: (2,'bravo') appears once.
    assert_eq!(rows, vec![
        vec![Value::Int64(1), Value::String("alpha".into())],
        vec![Value::Int64(2), Value::String("bravo".into())],
        vec![Value::Int64(3), Value::String("charlie".into())],
    ]);
}

#[test]
fn union_all_keeps_duplicates() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT id, name FROM t.public.a UNION ALL SELECT id, name FROM t.public.b"
    ).unwrap());
    assert_eq!(rows.len(), 4);
}

#[test]
fn union_three_arms() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.a (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.b (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.c (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.a(id) VALUES(1)",
        "INSERT INTO t.public.b(id) VALUES(2)",
        "INSERT INTO t.public.c(id) VALUES(3)",
    ] { db.execute(sql).unwrap(); }
    let (_, rows) = rows_of(db.execute(
        "SELECT id FROM t.public.a UNION SELECT id FROM t.public.b UNION SELECT id FROM t.public.c"
    ).unwrap());
    assert_eq!(rows, vec![
        vec![Value::Int64(1)],
        vec![Value::Int64(2)],
        vec![Value::Int64(3)],
    ]);
}

#[test]
fn union_mixed_all_and_dedup() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // a UNION ALL b UNION b — the second UNION (no ALL) dedups
    // the whole accumulated result.
    let (_, rows) = rows_of(db.execute(
        "SELECT id, name FROM t.public.a UNION ALL SELECT id, name FROM t.public.b UNION SELECT id, name FROM t.public.b"
    ).unwrap());
    // Distinct ids: 1, 2, 3.
    assert_eq!(rows.len(), 3);
}

#[test]
fn union_column_count_mismatch_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.a UNION SELECT id, name FROM t.public.b"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn union_with_where_on_each_arm() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT id FROM t.public.a WHERE id = 1 UNION ALL SELECT id FROM t.public.b WHERE id = 3"
    ).unwrap());
    assert_eq!(rows, vec![
        vec![Value::Int64(1)],
        vec![Value::Int64(3)],
    ]);
}

#[test]
fn union_all_empty_arm() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.a (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.b (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.a(id) VALUES(1), (2)",
    ] { db.execute(sql).unwrap(); }
    let (_, rows) = rows_of(db.execute(
        "SELECT id FROM t.public.a UNION ALL SELECT id FROM t.public.b"
    ).unwrap());
    assert_eq!(rows.len(), 2);
}

#[test]
fn union_both_arms_empty() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.a (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.b (id BIGINT NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    let (_, rows) = rows_of(db.execute(
        "SELECT id FROM t.public.a UNION SELECT id FROM t.public.b"
    ).unwrap());
    assert!(rows.is_empty());
}

#[test]
fn union_inside_quoted_literal_is_not_a_separator() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"k":"UNION of strings"}')"#,
    ] { db.execute(sql).unwrap(); }
    // The literal 'UNION of strings' must NOT be split as a
    // top-level UNION separator. The JSON-path WHERE accepts
    // non-PK predicates, so this exercises the splitter safely.
    let (_, rows) = rows_of(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'k' = 'UNION of strings'"
    ).unwrap());
    assert_eq!(rows.len(), 1);
}
