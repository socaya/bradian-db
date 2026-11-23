//! ADR 0241 — ALTER COLUMN SET NOT NULL [BACKFILL lit].

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn open(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, note TEXT, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    db
}

#[test]
fn set_not_null_succeeds_when_no_nulls() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    db.execute("INSERT INTO t.public.items(id, note) VALUES(1, 'x'), (2, 'y')").unwrap();
    db.execute("ALTER TABLE t.public.items ALTER COLUMN note SET NOT NULL").unwrap();
    // Inserting NULL into note now rejects.
    let err = db.execute("INSERT INTO t.public.items(id, note) VALUES(3, NULL)").unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

#[test]
fn set_not_null_rejects_when_nulls_exist() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    db.execute("INSERT INTO t.public.items(id, note) VALUES(1, 'x'), (2, NULL)").unwrap();
    let err = db.execute("ALTER TABLE t.public.items ALTER COLUMN note SET NOT NULL").unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

#[test]
fn set_not_null_with_backfill_replaces_nulls() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    db.execute("INSERT INTO t.public.items(id, note) VALUES(1, 'x'), (2, NULL), (3, NULL)").unwrap();
    db.execute("ALTER TABLE t.public.items ALTER COLUMN note SET NOT NULL BACKFILL 'default'").unwrap();
    match db.execute("SELECT id, note FROM t.public.items WHERE id = 2").unwrap() {
        SqlResultSet::Rows { rows, .. } => {
            assert_eq!(rows[0][1], Value::String("default".into()));
        }
        _ => panic!("expected rows"),
    }
    // Subsequent NULL insert rejects.
    let err = db.execute("INSERT INTO t.public.items(id, note) VALUES(4, NULL)").unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

#[test]
fn set_not_null_empty_table_succeeds() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    db.execute("ALTER TABLE t.public.items ALTER COLUMN note SET NOT NULL").unwrap();
    // Inserting NULL rejects.
    let err = db.execute("INSERT INTO t.public.items(id, note) VALUES(1, NULL)").unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

#[test]
fn set_not_null_on_already_not_null_rejected() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    let err = db.execute("ALTER TABLE t.public.items ALTER COLUMN id SET NOT NULL").unwrap_err();
    // Either Catalog or Unsupported is acceptable depending on the
    // path that reports already-not-null. Assert that it errors.
    let _ = err;
}

#[test]
fn set_not_null_on_unknown_column_rejected() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    let err = db.execute("ALTER TABLE t.public.items ALTER COLUMN missing SET NOT NULL").unwrap_err();
    assert!(matches!(err, SqlError::Resolution(_)));
}

#[test]
fn backfill_literal_type_mismatch_rejects() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    db.execute("INSERT INTO t.public.items(id, note) VALUES(1, NULL)").unwrap();
    // note is TEXT; integer BACKFILL rejects at coerce.
    let err = db.execute(
        "ALTER TABLE t.public.items ALTER COLUMN note SET NOT NULL BACKFILL 42"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}
