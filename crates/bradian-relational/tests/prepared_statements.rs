//! ADR 0100 — prepared-statement ergonomics smoke tests.

use bradian_relational::sql::{PreparedStatement, SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn open(dir: &std::path::Path) -> SqlEngine {
    SqlEngine::open(dir).unwrap()
}

fn setup(db: &SqlEngine) {
    db.execute("CREATE DATABASE test").unwrap();
    db.execute("CREATE SCHEMA test.public").unwrap();
    db.execute("CREATE TABLE test.public.items(id BIGINT NOT NULL, label VARCHAR(255), PRIMARY KEY(id))").unwrap();
}

#[test]
fn prepare_counts_placeholders_correctly() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    let p: PreparedStatement = db.prepare("SELECT ?, ?, ?").unwrap();
    assert_eq!(p.param_count(), 3);
    assert_eq!(p.template(), "SELECT ?, ?, ?");
}

#[test]
fn prepare_ignores_question_marks_inside_string_literals() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    let p = db.prepare("SELECT 'is ?' AS x WHERE id = ?").unwrap();
    assert_eq!(p.param_count(), 1, "the ? inside '…' must not count");
}

#[test]
fn prepare_rejects_unterminated_string_literal() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    let err = db.prepare("SELECT 'oops").unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)));
}

#[test]
fn execute_binds_i64_and_string_params() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    setup(&db);
    let ins = db.prepare("INSERT INTO test.public.items(id, label) VALUES(?, ?)").unwrap();
    ins.execute(&db, &[Value::Int64(1), Value::String("alpha".into())]).unwrap();
    ins.execute(&db, &[Value::Int64(2), Value::String("beta".into())]).unwrap();
    // The ADR 0100 template renders the SQL; verify by running a SELECT.
    let sel = db.execute("SELECT label FROM test.public.items WHERE id = 1").unwrap();
    match sel {
        SqlResultSet::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], Value::String("alpha".into()));
        }
        other => panic!("expected rows; got {other:?}"),
    }
}

#[test]
fn execute_rejects_param_count_mismatch() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    let p = db.prepare("SELECT ?, ?").unwrap();
    let err = p.execute(&db, &[Value::Int64(1)]).unwrap_err();
    match err {
        SqlError::Unsupported(msg) => assert!(msg.contains("prepared statement needs 2")),
        other => panic!("expected Unsupported; got {other:?}"),
    }
}

#[test]
fn bind_escapes_single_quotes_in_string_params() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    let p = db.prepare("INSERT INTO x(v) VALUES(?)").unwrap();
    let rendered = p.bind(&[Value::String("it's fine".into())]).unwrap();
    assert!(rendered.contains("'it''s fine'"),
        "single quote must be doubled per SQL escape; got: {rendered}");
}
