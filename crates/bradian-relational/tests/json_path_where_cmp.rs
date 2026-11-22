//! ADR 0195 — non-equality WHERE on JSON path.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"level":1,"name":"alice"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"level":3,"name":"bob"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"level":5,"name":"carol"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(4, '{"level":7,"name":"derek"}')"#,
    ] { db.execute(sql).unwrap(); }
    db
}

fn ids(set: SqlResultSet) -> Vec<i64> {
    match set {
        SqlResultSet::Rows { rows, .. } => rows.into_iter().map(|r| match r[0] {
            Value::Int64(n) => n,
            _ => panic!("expected Int64"),
        }).collect(),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn where_json_path_less_than_text() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        ids(db.execute("SELECT id FROM t.public.docs WHERE body ->> 'name' < 'carol'").unwrap()),
        vec![1, 2],
    );
}

#[test]
fn where_json_path_greater_than_text() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        ids(db.execute("SELECT id FROM t.public.docs WHERE body ->> 'name' > 'bob'").unwrap()),
        vec![3, 4],
    );
}

#[test]
fn where_json_path_less_or_equal() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        ids(db.execute("SELECT id FROM t.public.docs WHERE body ->> 'name' <= 'bob'").unwrap()),
        vec![1, 2],
    );
}

#[test]
fn where_json_path_greater_or_equal() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        ids(db.execute("SELECT id FROM t.public.docs WHERE body ->> 'name' >= 'carol'").unwrap()),
        vec![3, 4],
    );
}

#[test]
fn where_json_path_not_equal() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        ids(db.execute("SELECT id FROM t.public.docs WHERE body ->> 'name' <> 'alice'").unwrap()),
        vec![2, 3, 4],
    );
}

#[test]
fn where_json_path_numeric_text_compare_is_lexicographic() {
    // ->> compares as text — SQL-standard lexicographic ordering.
    // '7' > '10' lexicographically but not numerically.
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"n":"2"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"n":"10"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"n":"7"}')"#,
    ] { db.execute(sql).unwrap(); }
    assert_eq!(
        ids(db.execute("SELECT id FROM t.public.docs WHERE body ->> 'n' > '5'").unwrap()),
        vec![3],
    );
}

#[test]
fn where_json_path_container_form_with_comparison() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        ids(db.execute("SELECT id FROM t.public.docs WHERE body #>> '{name}' < 'carol'").unwrap()),
        vec![1, 2],
    );
}

#[test]
fn where_json_path_missing_key_never_matches_non_equality() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // id=1 has no 'nope' key; no row should be returned for `> ''`
    // or `< 'z'`.
    assert_eq!(
        ids(db.execute("SELECT id FROM t.public.docs WHERE body ->> 'nope' < 'z'").unwrap()),
        Vec::<i64>::new(),
    );
}

#[test]
fn where_json_path_equality_still_works_after_refactor() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        ids(db.execute("SELECT id FROM t.public.docs WHERE body ->> 'name' = 'bob'").unwrap()),
        vec![2],
    );
}

#[test]
fn where_json_path_bad_op_fails_cleanly() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // `~` is not a comparison operator in this grammar.
    let err = db.execute("SELECT id FROM t.public.docs WHERE body ->> 'name' ~ 'bob'").unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_) | SqlError::Unsupported(_)),
        "unknown op must fail cleanly; got {err:?}");
}
