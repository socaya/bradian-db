//! ADR 0113 — SQL grammar primitive extensions.
//!
//! JSON / TimestampTz / Uuid types reachable from CREATE
//! TABLE; TRUNCATE TABLE; versioned profile string.

use bradian_relational::sql::{SqlEngine, SqlResultSet, SQL_PROFILE_VERSION};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn open(dir: &std::path::Path) -> SqlEngine { SqlEngine::open(dir).unwrap() }

#[test]
fn profile_version_is_stable_string() {
    assert!(SQL_PROFILE_VERSION.starts_with("bradian-sql-"));
}

#[test]
fn json_column_accepts_valid_json_literal() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for s in ["CREATE DATABASE t", "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs(id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))"
    ] { db.execute(s).unwrap(); }
    db.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"ou":"abc","level":3}')"#).unwrap();
    let r = db.execute("SELECT body FROM t.public.docs WHERE id = 1").unwrap();
    match r {
        SqlResultSet::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            let v = &rows[0][0];
            // ADR 0173 — JSON columns now round-trip as the
            // dedicated Value::Json variant (previously
            // Value::String under ADR 0113).
            match v {
                Value::Json(s) => {
                    let parsed: serde_json::Value = serde_json::from_str(s).unwrap();
                    assert_eq!(parsed["ou"], "abc");
                    assert_eq!(parsed["level"], 3);
                }
                other => panic!("expected Json; got {other:?}"),
            }
        }
        other => panic!("expected rows; got {other:?}"),
    }
}

#[test]
fn json_column_rejects_invalid_json_literal() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for s in ["CREATE DATABASE t", "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs(id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))"
    ] { db.execute(s).unwrap(); }
    let err = db.execute("INSERT INTO t.public.docs(id, body) VALUES(1, 'not-json{')")
        .unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("JSON") || msg.contains("Type"),
        "expected JSON/Type error; got: {msg}");
}

#[test]
fn uuid_column_accepts_hex_literal() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for s in ["CREATE DATABASE t", "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.e(id UUID NOT NULL, PRIMARY KEY(id))"
    ] { db.execute(s).unwrap(); }
    // 32 hex chars — RFC 4122-style representation.
    db.execute("INSERT INTO t.public.e(id) VALUES('550e8400e29b41d4a716446655440000')").unwrap();
    // Dashed form also accepted.
    db.execute("INSERT INTO t.public.e(id) VALUES('550e8400-e29b-41d4-a716-446655440001')").unwrap();
    let err = db.execute("INSERT INTO t.public.e(id) VALUES('not-a-uuid')").unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("Type") || msg.contains("literal"),
        "invalid uuid must error; got: {msg}");
}

#[test]
fn timestamptz_column_accepts_integer_micros() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for s in ["CREATE DATABASE t", "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.events(id BIGINT NOT NULL, at TIMESTAMPTZ NOT NULL, PRIMARY KEY(id))"
    ] { db.execute(s).unwrap(); }
    db.execute("INSERT INTO t.public.events(id, at) VALUES(1, 1731700000000000)").unwrap();
    let r = db.execute("SELECT at FROM t.public.events WHERE id = 1").unwrap();
    match r {
        SqlResultSet::Rows { rows, .. } => {
            match rows[0][0] {
                Value::TimestampTz(micros) => assert_eq!(micros, 1731700000000000),
                _ => panic!("expected TimestampTz"),
            }
        }
        other => panic!("expected rows; got {other:?}"),
    }
}

#[test]
fn truncate_table_removes_every_row() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for s in ["CREATE DATABASE t", "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items(id BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id) VALUES(1)",
        "INSERT INTO t.public.items(id) VALUES(2)",
        "INSERT INTO t.public.items(id) VALUES(3)",
    ] { db.execute(s).unwrap(); }
    let r = db.execute("TRUNCATE TABLE t.public.items").unwrap();
    match r {
        SqlResultSet::Command { affected } => assert_eq!(affected, 3),
        other => panic!("expected Command; got {other:?}"),
    }
    // And subsequent inserts work — schema preserved.
    db.execute("INSERT INTO t.public.items(id) VALUES(4)").unwrap();
    let r = db.execute("SELECT id FROM t.public.items WHERE id = 4").unwrap();
    match r {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows.len(), 1),
        other => panic!("expected rows; got {other:?}"),
    }
}

#[test]
fn truncate_on_empty_table_is_idempotent() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for s in ["CREATE DATABASE t", "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.empty(id BIGINT NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(s).unwrap(); }
    let r = db.execute("TRUNCATE TABLE t.public.empty").unwrap();
    match r {
        SqlResultSet::Command { affected } => assert_eq!(affected, 0),
        other => panic!("expected Command; got {other:?}"),
    }
}
