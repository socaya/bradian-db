//! ADR 0259 — expression DEFAULT (CURRENT_TIMESTAMP / NOW()).

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, created_at TIMESTAMPTZ, note TEXT, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    (dir, db)
}

#[test]
fn set_default_current_timestamp_fills_omitted_column() {
    let (_dir, db) = seed();
    db.execute("ALTER TABLE t.public.items ALTER COLUMN created_at SET DEFAULT CURRENT_TIMESTAMP")
        .unwrap();
    db.execute("INSERT INTO t.public.items(id, note) VALUES(1, 'a')").unwrap();
    let rs = db.execute("SELECT created_at FROM t.public.items WHERE id = 1").unwrap();
    match rs {
        SqlResultSet::Rows { rows, .. } => {
            assert!(matches!(rows[0][0], Value::TimestampTz(n) if n > 0),
                "expected non-zero timestamp, got {:?}", rows[0][0]);
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn now_grammar_is_equivalent_to_current_timestamp() {
    let (_dir, db) = seed();
    db.execute("ALTER TABLE t.public.items ALTER COLUMN created_at SET DEFAULT NOW()")
        .unwrap();
    db.execute("INSERT INTO t.public.items(id, note) VALUES(1, 'a')").unwrap();
    let rs = db.execute("SELECT created_at FROM t.public.items WHERE id = 1").unwrap();
    match rs {
        SqlResultSet::Rows { rows, .. } =>
            assert!(matches!(rows[0][0], Value::TimestampTz(_))),
        other => panic!("{other:?}"),
    }
}

#[test]
fn now_without_parens_rejected() {
    let (_dir, db) = seed();
    let err = db.execute("ALTER TABLE t.public.items ALTER COLUMN created_at SET DEFAULT NOW")
        .unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)), "got {err:?}");
}

#[test]
fn current_timestamp_on_non_timestamp_column_rejected() {
    let (_dir, db) = seed();
    let err = db.execute("ALTER TABLE t.public.items ALTER COLUMN note SET DEFAULT CURRENT_TIMESTAMP")
        .unwrap_err();
    assert!(matches!(err, SqlError::Catalog(_)), "got {err:?}");
}

#[test]
fn drop_default_removes_expression_default() {
    let (_dir, db) = seed();
    db.execute("ALTER TABLE t.public.items ALTER COLUMN created_at SET DEFAULT CURRENT_TIMESTAMP")
        .unwrap();
    db.execute("ALTER TABLE t.public.items ALTER COLUMN created_at DROP DEFAULT").unwrap();
    // Column is nullable so omitting is still allowed; now it becomes NULL.
    db.execute("INSERT INTO t.public.items(id, note) VALUES(1, 'a')").unwrap();
    let rs = db.execute("SELECT created_at FROM t.public.items WHERE id = 1").unwrap();
    match rs {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Null),
        other => panic!("{other:?}"),
    }
}

#[test]
fn omit_column_without_default_or_null_rejected() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        // Every column NOT NULL and no defaults.
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, note TEXT NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    let err = db.execute("INSERT INTO t.public.items(id) VALUES(1)").unwrap_err();
    assert!(matches!(err, SqlError::Type(_)), "got {err:?}");
}

#[test]
fn literal_default_still_works_on_omit() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, note TEXT NOT NULL, PRIMARY KEY(id))",
        "ALTER TABLE t.public.items ALTER COLUMN note SET DEFAULT 'auto'",
    ] { db.execute(sql).unwrap(); }
    db.execute("INSERT INTO t.public.items(id) VALUES(1)").unwrap();
    let rs = db.execute("SELECT note FROM t.public.items WHERE id = 1").unwrap();
    match rs {
        SqlResultSet::Rows { rows, .. } =>
            assert_eq!(rows[0][0], Value::String("auto".into())),
        other => panic!("{other:?}"),
    }
}

#[test]
fn expression_default_overrides_literal_default() {
    let (_dir, db) = seed();
    // Literal set first, then expression — expression wins.
    db.execute("ALTER TABLE t.public.items ALTER COLUMN created_at SET DEFAULT 42").unwrap();
    db.execute("ALTER TABLE t.public.items ALTER COLUMN created_at SET DEFAULT CURRENT_TIMESTAMP")
        .unwrap();
    db.execute("INSERT INTO t.public.items(id, note) VALUES(1, 'a')").unwrap();
    let rs = db.execute("SELECT created_at FROM t.public.items WHERE id = 1").unwrap();
    match rs {
        SqlResultSet::Rows { rows, .. } => {
            // Must be the current time, not 42.
            assert!(matches!(rows[0][0], Value::TimestampTz(n) if n > 1_000_000));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn default_persists_across_reopen() {
    let dir = tempdir().unwrap();
    {
        let db = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, created_at TIMESTAMPTZ, PRIMARY KEY(id))",
            "ALTER TABLE t.public.items ALTER COLUMN created_at SET DEFAULT CURRENT_TIMESTAMP",
        ] { db.execute(sql).unwrap(); }
    }
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("INSERT INTO t.public.items(id) VALUES(1)").unwrap();
    let rs = db.execute("SELECT created_at FROM t.public.items WHERE id = 1").unwrap();
    match rs {
        SqlResultSet::Rows { rows, .. } =>
            assert!(matches!(rows[0][0], Value::TimestampTz(n) if n > 0)),
        other => panic!("{other:?}"),
    }
}
