//! ADR 0211 — ESCAPE clause for LIKE / ILIKE.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"s":"100%"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"s":"100 pct"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"s":"a_b"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(4, '{"s":"aXb"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(5, '{"s":"c\\d"}')"#,
    ] { db.execute(sql).unwrap(); }
    db
}

fn ids(rs: SqlResultSet) -> Vec<i64> {
    match rs {
        SqlResultSet::Rows { rows, .. } => rows.into_iter()
            .map(|r| match r[0] { Value::Int64(v) => v, _ => panic!("expected id") })
            .collect(),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn escape_quotes_literal_percent() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Pattern `100\%` with escape `\` matches the literal "100%".
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 's' LIKE '100\\%' ESCAPE '\\'"
    ).unwrap());
    assert_eq!(got, vec![1]);
}

#[test]
fn escape_quotes_literal_underscore() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Pattern `a\_b` with escape `\` matches exactly "a_b".
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 's' LIKE 'a\\_b' ESCAPE '\\'"
    ).unwrap());
    assert_eq!(got, vec![3]);
}

#[test]
fn escape_quotes_itself() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Row 5's s is "c\d". Pattern `c\\d` with escape `\` matches "c\d".
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 's' LIKE 'c\\\\d' ESCAPE '\\'"
    ).unwrap());
    assert_eq!(got, vec![5]);
}

#[test]
fn without_escape_percent_still_wildcard() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // No ESCAPE; `100%` is prefix-"100".
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 's' LIKE '100%'"
    ).unwrap());
    assert_eq!(got, vec![1, 2]);
}

#[test]
fn escape_requires_single_char() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 's' LIKE 'a%' ESCAPE 'ab'"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)));
}

#[test]
fn escape_empty_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 's' LIKE 'a%' ESCAPE ''"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)));
}

#[test]
fn not_like_with_escape() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Rows whose s is NOT literally "100%".
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 's' NOT LIKE '100\\%' ESCAPE '\\'"
    ).unwrap());
    assert_eq!(got, vec![2, 3, 4, 5]);
}

#[test]
fn ilike_with_escape() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"s":"50%"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"s":"abc"}')"#,
    ] { db.execute(sql).unwrap(); }
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 's' ILIKE '50\\%' ESCAPE '\\'"
    ).unwrap());
    assert_eq!(got, vec![1]);
}

#[test]
fn not_ilike_with_escape() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Rows whose lowercase s is NOT literally "100%".
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 's' NOT ILIKE '100\\%' ESCAPE '\\'"
    ).unwrap());
    assert_eq!(got, vec![2, 3, 4, 5]);
}

#[test]
fn escape_char_alone_at_end_of_pattern_fails() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // A dangling escape char with no following literal does not
    // match anything (returns no rows, not a parse error).
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 's' LIKE 'a\\' ESCAPE '\\'"
    ).unwrap());
    assert!(got.is_empty());
}

#[test]
fn custom_escape_character() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Use `#` as the escape so we don't need to double-escape.
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 's' LIKE '100#%' ESCAPE '#'"
    ).unwrap());
    assert_eq!(got, vec![1]);
}

#[test]
fn like_without_escape_still_works() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 's' LIKE 'a_b'"
    ).unwrap());
    // `_` is wildcard: matches both a_b and aXb.
    assert_eq!(got, vec![3, 4]);
}
