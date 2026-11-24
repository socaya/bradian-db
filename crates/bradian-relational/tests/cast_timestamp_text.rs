//! ADR 0263 — TIMESTAMPTZ → TEXT renders ISO-8601.

use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, ts TIMESTAMPTZ, PRIMARY KEY(id))",
        // 2024-01-15T10:30:00Z = 1705314600000000 unix micros.
        "INSERT INTO t.public.items(id, ts) VALUES(1, '2024-01-15T10:30:00Z')",
        // Epoch.
        "INSERT INTO t.public.items(id, ts) VALUES(2, 0)",
        // Pre-epoch: negative micros.
        "INSERT INTO t.public.items(id, ts) VALUES(3, -1000000)",
    ] { db.execute(sql).unwrap(); }
    (dir, db)
}

fn first(rs: SqlResultSet) -> Vec<Value> {
    match rs { SqlResultSet::Rows { rows, .. } => rows.into_iter().next().unwrap(),
               other => panic!("{other:?}") }
}

#[test]
fn render_epoch() {
    let (_dir, db) = seed();
    let row = first(db.execute(
        "SELECT CAST(ts AS TEXT) FROM t.public.items WHERE id = 2"
    ).unwrap());
    assert_eq!(row, vec![Value::String("1970-01-01T00:00:00.000000+00:00".into())]);
}

#[test]
fn render_recent_timestamp() {
    let (_dir, db) = seed();
    let row = first(db.execute(
        "SELECT CAST(ts AS TEXT) FROM t.public.items WHERE id = 1"
    ).unwrap());
    assert_eq!(row, vec![Value::String("2024-01-15T10:30:00.000000+00:00".into())]);
}

#[test]
fn render_pre_epoch() {
    let (_dir, db) = seed();
    let row = first(db.execute(
        "SELECT CAST(ts AS TEXT) FROM t.public.items WHERE id = 3"
    ).unwrap());
    assert_eq!(row, vec![Value::String("1969-12-31T23:59:59.000000+00:00".into())]);
}

#[test]
fn round_trip_iso8601() {
    // CAST(TEXT→TIMESTAMPTZ) composed with CAST(TIMESTAMPTZ→TEXT)
    // must produce an equivalent timestamp (same micros).
    let (_dir, db) = seed();
    let row = first(db.execute(
        "SELECT CAST(CAST(CAST(ts AS TEXT) AS TIMESTAMPTZ) AS TEXT) FROM t.public.items WHERE id = 1"
    ).unwrap());
    assert_eq!(row, vec![Value::String("2024-01-15T10:30:00.000000+00:00".into())]);
}

#[test]
fn render_includes_fractional_micros() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, ts TIMESTAMPTZ, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, ts) VALUES(1, '2024-01-15T10:30:00.123456Z')",
    ] { db.execute(sql).unwrap(); }
    let row = first(db.execute(
        "SELECT CAST(ts AS TEXT) FROM t.public.items WHERE id = 1"
    ).unwrap());
    assert_eq!(row, vec![Value::String("2024-01-15T10:30:00.123456+00:00".into())]);
}
