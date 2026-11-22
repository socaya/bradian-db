//! ADR 0186 — JSON path operators `->` and `->>` in SqlEngine SELECT.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn open(dir: &std::path::Path) -> SqlEngine { SqlEngine::open(dir).unwrap() }

fn seeded(db: &SqlEngine) {
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"name":"alice","level":3,"nested":{"k":42}}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"name":"bob","level":5}')"#,
    ] { db.execute(sql).unwrap(); }
}

#[test]
fn arrow_extracts_json_subvalue_preserving_json_variant() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seeded(&db);
    match db.execute("SELECT body -> 'nested' FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { columns, rows } => {
            assert_eq!(columns, vec!["body->'nested'".to_string()]);
            match &rows[0][0] {
                Value::Json(s) => {
                    let parsed: serde_json::Value = serde_json::from_str(s).unwrap();
                    assert_eq!(parsed["k"], 42);
                }
                other => panic!("expected Value::Json; got {other:?}"),
            }
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn arrow_arrow_extracts_as_text() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seeded(&db);
    match db.execute("SELECT body ->> 'name' FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { columns, rows } => {
            assert_eq!(columns, vec!["body->>'name'".to_string()]);
            match &rows[0][0] {
                Value::String(s) => assert_eq!(s, "alice"),
                other => panic!("expected Value::String; got {other:?}"),
            }
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn arrow_arrow_on_numeric_subvalue_renders_canonical_text() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seeded(&db);
    match db.execute("SELECT body ->> 'level' FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => {
            match &rows[0][0] {
                Value::String(s) => assert_eq!(s, "3"),
                other => panic!("expected Value::String; got {other:?}"),
            }
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn missing_key_returns_null() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seeded(&db);
    match db.execute("SELECT body -> 'nope' FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Null),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn path_combines_with_other_projections_and_scan() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seeded(&db);
    match db.execute("SELECT id, body ->> 'name' FROM t.public.docs").unwrap() {
        SqlResultSet::Rows { columns, rows } => {
            assert_eq!(columns, vec!["id".to_string(), "body->>'name'".to_string()]);
            assert_eq!(rows.len(), 2);
            let names: std::collections::BTreeSet<String> = rows.iter().map(|r|
                match &r[1] { Value::String(s) => s.clone(), _ => String::new() }
            ).collect();
            assert!(names.contains("alice"));
            assert!(names.contains("bob"));
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn path_on_non_json_column_fails_cleanly() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.a (id BIGINT NOT NULL, label TEXT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.a(id, label) VALUES(1, 'plain-text')",
    ] { db.execute(sql).unwrap(); }
    // The parser accepts the clause, but the stored cell is
    // not valid JSON — runtime extraction errors.
    let err = db.execute("SELECT label -> 'anything' FROM t.public.a WHERE id=1").unwrap_err();
    assert!(matches!(err, SqlError::Type(_)),
        "non-JSON content should produce Type error; got {err:?}");
}

#[test]
fn path_rejects_non_text_non_integer_key_literal() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seeded(&db);
    // ADR 0191 — array indices are now accepted. A non-integer
    // numeric or a bare identifier still fails cleanly.
    let err = db.execute(
        "SELECT body -> 4.2 FROM t.public.docs WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)),
        "float as JSON path segment should be rejected; got {err:?}");
    let err = db.execute(
        "SELECT body -> -1 FROM t.public.docs WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)),
        "negative index should be rejected; got {err:?}");
}

#[test]
fn path_cell_null_passes_through_as_null() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.n (id BIGINT NOT NULL, body JSON, PRIMARY KEY(id))",
        "INSERT INTO t.public.n(id, body) VALUES(1, NULL)",
    ] { db.execute(sql).unwrap(); }
    match db.execute("SELECT body -> 'k' FROM t.public.n WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Null),
        other => panic!("expected Rows; got {other:?}"),
    }
}
