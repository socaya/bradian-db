//! ADR 0240 — NOT prefix + parens in JSON-path WHERE.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"status":"open","owner":"alice","age":30}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"status":"open","owner":"bob","age":42}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"status":"closed","owner":"alice","age":35}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(4, '{"status":"open","owner":"carol","age":25}')"#,
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
fn not_prefix_inverts_scalar_predicate() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE NOT body ->> 'status' = 'open'"
    ).unwrap());
    assert_eq!(got, vec![3]);
}

#[test]
fn parens_force_or_over_and() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // (status=closed OR owner=carol) AND age >= 25
    // → id 3 (closed, 35), id 4 (open/carol, 25).
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE (body ->> 'status' = 'closed' OR body ->> 'owner' = 'carol') AND body -> 'age' >= 25"
    ).unwrap());
    assert_eq!(got, vec![3, 4]);
}

#[test]
fn not_around_or_expression() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // NOT (owner=bob OR owner=carol) → owner in {alice}; id 1, 3.
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE NOT (body ->> 'owner' = 'bob' OR body ->> 'owner' = 'carol')"
    ).unwrap());
    assert_eq!(got, vec![1, 3]);
}

#[test]
fn double_not_cancels() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE NOT NOT body ->> 'status' = 'open'"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 4]);
}

#[test]
fn not_around_between() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // NOT (age BETWEEN 30 AND 40) → age < 30 or > 40
    //   → id 2 (42), id 4 (25).
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE NOT body -> 'age' BETWEEN 30 AND 40"
    ).unwrap());
    assert_eq!(got, vec![2, 4]);
}

#[test]
fn not_around_in_list() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE NOT body ->> 'owner' IN ('alice', 'bob')"
    ).unwrap());
    assert_eq!(got, vec![4]);
}

#[test]
fn parens_with_nested_and_or() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // (status=open AND (owner=alice OR age > 40)) → id 1 (alice, open), id 2 (bob, 42).
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'status' = 'open' AND (body ->> 'owner' = 'alice' OR body -> 'age' > 40)"
    ).unwrap());
    assert_eq!(got, vec![1, 2]);
}

#[test]
fn not_of_in_equivalent_to_not_in() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let a = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE NOT body ->> 'owner' IN ('alice','carol')"
    ).unwrap());
    let b = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'owner' NOT IN ('alice','carol')"
    ).unwrap());
    assert_eq!(a, b);
}

#[test]
fn unbalanced_parens_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.docs WHERE (body ->> 'status' = 'open'"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)));
}
