//! ADR 0260 — ALTER COLUMN TYPE narrowing with implicit CAST.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

#[test]
fn narrow_bigint_to_int_when_values_fit() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, amount BIGINT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, amount) VALUES(1, 100)",
        "INSERT INTO t.public.items(id, amount) VALUES(2, 2000)",
    ] { db.execute(sql).unwrap(); }
    // Narrow BIGINT to INT (Int32).
    db.execute("ALTER TABLE t.public.items ALTER COLUMN amount TYPE INT").unwrap();
    let rs = db.execute("SELECT amount FROM t.public.items WHERE id = 1").unwrap();
    match rs {
        SqlResultSet::Rows { rows, .. } =>
            assert_eq!(rows[0][0], Value::Int32(100)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn narrow_rejects_when_any_row_overflows() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, amount BIGINT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, amount) VALUES(1, 100)",
        // Does not fit in i32.
        "INSERT INTO t.public.items(id, amount) VALUES(2, 9999999999)",
    ] { db.execute(sql).unwrap(); }
    let err = db.execute("ALTER TABLE t.public.items ALTER COLUMN amount TYPE INT")
        .unwrap_err();
    assert!(matches!(err, SqlError::Type(_)), "got {err:?}");
    // Column still BIGINT.
    let rs = db.execute("SELECT amount FROM t.public.items WHERE id = 2").unwrap();
    match rs {
        SqlResultSet::Rows { rows, .. } =>
            assert_eq!(rows[0][0], Value::Int64(9999999999)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn narrow_text_to_int_when_text_is_numeric() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, note TEXT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, note) VALUES(1, '42')",
        "INSERT INTO t.public.items(id, note) VALUES(2, '99')",
    ] { db.execute(sql).unwrap(); }
    db.execute("ALTER TABLE t.public.items ALTER COLUMN note TYPE BIGINT").unwrap();
    let rs = db.execute("SELECT note FROM t.public.items WHERE id = 1").unwrap();
    match rs {
        SqlResultSet::Rows { rows, .. } =>
            assert_eq!(rows[0][0], Value::Int64(42)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn narrow_text_to_int_rejects_when_any_text_is_non_numeric() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, note TEXT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, note) VALUES(1, '42')",
        "INSERT INTO t.public.items(id, note) VALUES(2, 'hello')",
    ] { db.execute(sql).unwrap(); }
    let err = db.execute("ALTER TABLE t.public.items ALTER COLUMN note TYPE BIGINT")
        .unwrap_err();
    assert!(matches!(err, SqlError::Type(_)), "got {err:?}");
}

#[test]
fn narrow_bigint_to_text() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, amount BIGINT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, amount) VALUES(1, 100)",
    ] { db.execute(sql).unwrap(); }
    db.execute("ALTER TABLE t.public.items ALTER COLUMN amount TYPE TEXT").unwrap();
    let rs = db.execute("SELECT amount FROM t.public.items WHERE id = 1").unwrap();
    match rs {
        SqlResultSet::Rows { rows, .. } =>
            assert_eq!(rows[0][0], Value::String("100".into())),
        other => panic!("{other:?}"),
    }
}

#[test]
fn narrow_persists_across_reopen() {
    let dir = tempdir().unwrap();
    {
        let db = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, amount BIGINT, PRIMARY KEY(id))",
            "INSERT INTO t.public.items(id, amount) VALUES(1, 100)",
            "ALTER TABLE t.public.items ALTER COLUMN amount TYPE INT",
        ] { db.execute(sql).unwrap(); }
    }
    let db = SqlEngine::open(dir.path()).unwrap();
    let rs = db.execute("SELECT amount FROM t.public.items WHERE id = 1").unwrap();
    match rs {
        SqlResultSet::Rows { rows, .. } =>
            assert_eq!(rows[0][0], Value::Int32(100)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn widen_still_works_without_row_scan() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, amount INT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, amount) VALUES(1, 100)",
        // INT → BIGINT is a widen; fast path.
        "ALTER TABLE t.public.items ALTER COLUMN amount TYPE BIGINT",
    ] { db.execute(sql).unwrap(); }
    let rs = db.execute("SELECT amount FROM t.public.items WHERE id = 1").unwrap();
    match rs {
        SqlResultSet::Rows { rows, .. } =>
            assert_eq!(rows[0][0], Value::Int64(100)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn narrow_rejects_when_column_is_pk_or_has_dependents() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    let err = db.execute("ALTER TABLE t.public.items ALTER COLUMN id TYPE INT").unwrap_err();
    assert!(matches!(err, SqlError::Catalog(_)), "got {err:?}");
}
