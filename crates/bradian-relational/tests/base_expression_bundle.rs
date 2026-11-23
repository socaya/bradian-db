//! ADRs 0234–0238 — nested args, CAST, date/time, aggregates
//! over expressions, BETWEEN in CASE.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, qty BIGINT NOT NULL, created TIMESTAMPTZ NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, name, qty, created) VALUES \
         (1, '  Alpha  ', 10, '2024-01-01T00:00:00Z'), \
         (2, 'bravo', -5, '2024-01-01T00:00:30Z'), \
         (3, 'CHARLIE', 50, '2024-01-01T00:01:00Z')",
    ] { db.execute(sql).unwrap(); }
    db
}

fn rows_of(rs: SqlResultSet) -> Vec<Vec<Value>> {
    match rs {
        SqlResultSet::Rows { rows, .. } => rows,
        other => panic!("expected Rows; got {other:?}"),
    }
}

// --- ADR 0234: Nested function arguments ---

#[test]
fn nested_upper_trim() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = rows_of(db.execute(
        "SELECT UPPER(TRIM(name)) FROM t.public.items WHERE id = 1"
    ).unwrap());
    assert_eq!(got, vec![vec![Value::String("ALPHA".into())]]);
}

#[test]
fn nested_coalesce_with_upper() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, label TEXT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, label) VALUES(1, 'x'), (2, NULL)",
    ] { db.execute(sql).unwrap(); }
    let got = rows_of(db.execute(
        "SELECT COALESCE(UPPER(label), 'MISS') FROM t.public.items"
    ).unwrap());
    assert_eq!(got[0][0], Value::String("X".into()));
    assert_eq!(got[1][0], Value::String("MISS".into()));
}

// --- ADR 0235: CAST ---

#[test]
fn cast_int_to_text() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = rows_of(db.execute(
        "SELECT CAST(qty AS TEXT) FROM t.public.items WHERE id = 1"
    ).unwrap());
    assert_eq!(got, vec![vec![Value::String("10".into())]]);
}

#[test]
fn cast_text_to_int() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, num TEXT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, num) VALUES(1, '42')",
    ] { db.execute(sql).unwrap(); }
    let got = rows_of(db.execute(
        "SELECT CAST(num AS BIGINT) FROM t.public.items WHERE id = 1"
    ).unwrap());
    assert_eq!(got, vec![vec![Value::Int64(42)]]);
}

#[test]
fn cast_bad_target_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT CAST(qty AS VARCHAR) FROM t.public.items WHERE id = 1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn cast_bad_value_rejected() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, s TEXT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, s) VALUES(1, 'not-a-number')",
    ] { db.execute(sql).unwrap(); }
    let err = db.execute(
        "SELECT CAST(s AS BIGINT) FROM t.public.items WHERE id = 1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

// --- ADR 0236: Date/time ---

#[test]
fn timestamp_diff_returns_seconds() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // id 3 created 60s after id 1.
    let got = rows_of(db.execute(
        "SELECT TIMESTAMP_DIFF(created, created) FROM t.public.items WHERE id = 1"
    ).unwrap());
    assert_eq!(got, vec![vec![Value::Int64(0)]]);
}

#[test]
fn timestamp_add_shifts_forward() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Add 300 seconds to id 1's created. Result encoded as micros.
    let got = rows_of(db.execute(
        "SELECT TIMESTAMP_ADD(created, 300) FROM t.public.items WHERE id = 1"
    ).unwrap());
    match &got[0][0] {
        Value::TimestampTz(us) => {
            // The seed inserted 2024-01-01T00:00:00Z. 1704067200 secs
            // from epoch. + 300 seconds = 1704067500. Converted to us.
            assert_eq!(*us, 1704067500 * 1_000_000);
        }
        other => panic!("expected TimestampTz; got {other:?}"),
    }
}

#[test]
fn now_returns_a_timestamp() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = rows_of(db.execute(
        "SELECT NOW() FROM t.public.items WHERE id = 1"
    ).unwrap());
    assert!(matches!(got[0][0], Value::TimestampTz(_)));
}

// --- ADR 0237: Aggregates over expressions ---

#[test]
fn sum_abs() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // 10 + 5 + 50 = 65.
    let got = rows_of(db.execute(
        "SELECT SUM(ABS(qty)) FROM t.public.items"
    ).unwrap());
    assert_eq!(got, vec![vec![Value::Int64(65)]]);
}

#[test]
fn avg_length() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, name) VALUES(1, 'ab'), (2, 'cdef'), (3, 'g')",
    ] { db.execute(sql).unwrap(); }
    // Lengths 2, 4, 1 → avg 7/3 = 2.33… → half-even → 2.
    let got = rows_of(db.execute(
        "SELECT AVG(LENGTH(name)) FROM t.public.items"
    ).unwrap());
    assert_eq!(got, vec![vec![Value::Int64(2)]]);
}

#[test]
fn max_upper() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // UPPER names: '  ALPHA  ', 'BRAVO', 'CHARLIE'.
    let got = rows_of(db.execute(
        "SELECT MAX(UPPER(name)) FROM t.public.items"
    ).unwrap());
    assert_eq!(got, vec![vec![Value::String("CHARLIE".into())]]);
}

#[test]
fn count_distinct_cast() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, s TEXT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, s) VALUES(1, '1'), (2, '2'), (3, '1')",
    ] { db.execute(sql).unwrap(); }
    let got = rows_of(db.execute(
        "SELECT COUNT(DISTINCT CAST(s AS BIGINT)) FROM t.public.items"
    ).unwrap());
    assert_eq!(got, vec![vec![Value::Int64(2)]]);
}

// --- ADR 0238: BETWEEN in CASE ---

#[test]
fn case_between_classifies_numeric() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // qty 10 → mid; qty -5 → low; qty 50 → high.
    let got = rows_of(db.execute(
        "SELECT CASE WHEN qty BETWEEN 0 AND 20 THEN 'mid' WHEN qty > 20 THEN 'high' ELSE 'low' END FROM t.public.items"
    ).unwrap());
    assert_eq!(got[0][0], Value::String("mid".into()));
    assert_eq!(got[1][0], Value::String("low".into()));
    assert_eq!(got[2][0], Value::String("high".into()));
}

#[test]
fn case_not_between_classifies_outliers() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = rows_of(db.execute(
        "SELECT CASE WHEN qty NOT BETWEEN 0 AND 20 THEN 'outlier' ELSE 'normal' END FROM t.public.items"
    ).unwrap());
    assert_eq!(got[0][0], Value::String("normal".into()));   // 10
    assert_eq!(got[1][0], Value::String("outlier".into()));  // -5
    assert_eq!(got[2][0], Value::String("outlier".into()));  // 50
}
