//! ADR 0180 — ISO 8601 text literals for TIMESTAMPTZ columns.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(root: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(root).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.events (id BIGINT NOT NULL, at TIMESTAMPTZ NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    db
}

fn select_at(db: &SqlEngine, id: i64) -> i64 {
    match db.execute(&format!("SELECT at FROM t.public.events WHERE id={id}")).unwrap() {
        SqlResultSet::Rows { rows, .. } => match rows[0][0] {
            Value::TimestampTz(micros) => micros,
            _ => panic!("expected TimestampTz"),
        }
        _ => panic!("expected Rows"),
    }
}

#[test]
fn iso_8601_zulu_epoch_round_trips_as_zero_micros() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("INSERT INTO t.public.events(id, at) VALUES(1, '1970-01-01T00:00:00Z')").unwrap();
    assert_eq!(select_at(&db, 1), 0);
}

#[test]
fn iso_8601_zulu_with_microseconds_preserves_exact_precision() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("INSERT INTO t.public.events(id, at) VALUES(1, '2026-11-21T12:00:00.123456Z')").unwrap();
    // 2026-11-21 12:00:00 UTC == 1795 ... micros; test doesn't need the
    // exact number; just confirm .123456 microseconds are encoded.
    let micros = select_at(&db, 1);
    assert_eq!(micros % 1_000_000, 123_456,
        "fractional microsecond component preserved exactly; got {micros}");
}

#[test]
fn iso_8601_positive_offset_shifts_back_to_utc() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // 10:00 at UTC+02:00 == 08:00 UTC.
    db.execute("INSERT INTO t.public.events(id, at) VALUES(1, '2026-11-21T10:00:00+02:00')").unwrap();
    db.execute("INSERT INTO t.public.events(id, at) VALUES(2, '2026-11-21T08:00:00Z')").unwrap();
    assert_eq!(select_at(&db, 1), select_at(&db, 2),
        "UTC+02:00 at 10:00 should equal UTC 08:00");
}

#[test]
fn iso_8601_negative_offset_shifts_forward_to_utc() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // 08:00 at UTC-05:00 == 13:00 UTC.
    db.execute("INSERT INTO t.public.events(id, at) VALUES(1, '2026-11-21T08:00:00-05:00')").unwrap();
    db.execute("INSERT INTO t.public.events(id, at) VALUES(2, '2026-11-21T13:00:00Z')").unwrap();
    assert_eq!(select_at(&db, 1), select_at(&db, 2));
}

#[test]
fn iso_8601_integer_micros_still_accepted() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("INSERT INTO t.public.events(id, at) VALUES(1, 1731700000000000)").unwrap();
    assert_eq!(select_at(&db, 1), 1731700000000000);
}

#[test]
fn iso_8601_rejects_malformed_literals() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    for bad in [
        "'not a timestamp'",
        "'2026/11/21T12:00:00Z'",
        "'2026-11-21 12:00:00'",      // missing TZ designator
        "'2026-13-21T12:00:00Z'",     // month 13
        "'2026-11-21T25:00:00Z'",     // hour 25
        "'2026-11-21T12:00:00+99:00'",// bad offset
        "'2026-11-21T12:00:00.1234567Z'", // too many fractional digits
    ] {
        let err = db.execute(&format!(
            "INSERT INTO t.public.events(id, at) VALUES(1, {bad})"
        )).unwrap_err();
        assert!(matches!(err, SqlError::Type(_)),
            "expected SqlError::Type for {bad}; got {err:?}");
    }
}

#[test]
fn iso_8601_leap_second_minute_is_accepted() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // ISO 8601 permits 60 for the seconds field (positive leap
    // second). Many libraries reject this; Bradian normalizes
    // by accepting without adjustment — an operator who wants
    // strict POSIX semantics can clamp upstream.
    db.execute("INSERT INTO t.public.events(id, at) VALUES(1, '2016-12-31T23:59:60Z')").unwrap();
}
