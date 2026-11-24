//! ADR 0262 — DECIMAL(p,s) CAST target.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, n INT, s TEXT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, n, s) VALUES(1, 42, '123.45')",
        "INSERT INTO t.public.items(id, n, s) VALUES(2, 0, '0')",
        "INSERT INTO t.public.items(id, n, s) VALUES(3, -5, '-99.9')",
    ] { db.execute(sql).unwrap(); }
    (dir, db)
}

fn first(rs: SqlResultSet) -> Vec<Value> {
    match rs { SqlResultSet::Rows { rows, .. } => rows.into_iter().next().unwrap(),
               other => panic!("{other:?}") }
}

#[test]
fn cast_int_to_decimal_adds_scale_zeros() {
    let (_dir, db) = seed();
    let row = first(db.execute(
        "SELECT CAST(n AS DECIMAL(10, 2)) FROM t.public.items WHERE id = 1"
    ).unwrap());
    // 42 → 42.00 → stored as 4200 with scale 2.
    assert_eq!(row, vec![Value::Decimal(4200)]);
}

#[test]
fn cast_text_decimal_to_decimal() {
    let (_dir, db) = seed();
    let row = first(db.execute(
        "SELECT CAST(s AS DECIMAL(10, 2)) FROM t.public.items WHERE id = 1"
    ).unwrap());
    // '123.45' → 12345.
    assert_eq!(row, vec![Value::Decimal(12345)]);
}

#[test]
fn cast_negative_text_decimal() {
    let (_dir, db) = seed();
    let row = first(db.execute(
        "SELECT CAST(s AS DECIMAL(10, 2)) FROM t.public.items WHERE id = 3"
    ).unwrap());
    // '-99.9' at scale 2 → -9990.
    assert_eq!(row, vec![Value::Decimal(-9990)]);
}

#[test]
fn cast_text_with_too_many_fraction_digits_rejects() {
    let (_dir, db) = seed();
    let err = db.execute(
        "SELECT CAST('0.999' AS DECIMAL(5, 2)) FROM t.public.items WHERE id = 1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)), "got {err:?}");
}

#[test]
fn cast_overflow_rejects() {
    let (_dir, db) = seed();
    let err = db.execute(
        "SELECT CAST('999999.99' AS DECIMAL(5, 2)) FROM t.public.items WHERE id = 1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)), "got {err:?}");
}

#[test]
fn cast_int_too_wide_for_precision() {
    let (_dir, db) = seed();
    // 42 at scale 5 → 4200000; precision 6 bound is 10^6 = 1000000; overflow.
    let err = db.execute(
        "SELECT CAST(n AS DECIMAL(6, 5)) FROM t.public.items WHERE id = 1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)), "got {err:?}");
}

#[test]
fn cast_zero() {
    let (_dir, db) = seed();
    let row = first(db.execute(
        "SELECT CAST(n AS DECIMAL(5, 2)) FROM t.public.items WHERE id = 2"
    ).unwrap());
    assert_eq!(row, vec![Value::Decimal(0)]);
}

#[test]
fn cast_decimal_to_decimal_same_scale() {
    let (_dir, db) = seed();
    // Chain a decimal through a decimal cast.
    let row = first(db.execute(
        "SELECT CAST(CAST(n AS DECIMAL(10, 2)) AS DECIMAL(5, 2)) FROM t.public.items WHERE id = 1"
    ).unwrap());
    assert_eq!(row, vec![Value::Decimal(4200)]);
}
