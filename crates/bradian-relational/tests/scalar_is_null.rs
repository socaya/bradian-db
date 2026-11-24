//! ADR 0244 — bare-column scalar IS [NOT] NULL and
//! IS [NOT] DISTINCT FROM on non-JSON columns.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        // id is PK (NOT NULL); note is nullable.
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, note TEXT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, note) VALUES(1, 'alpha')",
        "INSERT INTO t.public.items(id, note) VALUES(2, NULL)",
        "INSERT INTO t.public.items(id, note) VALUES(3, 'beta')",
        "INSERT INTO t.public.items(id, note) VALUES(4, NULL)",
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
fn bare_column_is_null() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE note IS NULL"
    ).unwrap());
    assert_eq!(got, vec![2, 4]);
}

#[test]
fn bare_column_is_not_null() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE note IS NOT NULL"
    ).unwrap());
    assert_eq!(got, vec![1, 3]);
}

#[test]
fn bare_column_is_distinct_from_non_null_literal_matches_nulls_and_non_equal() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE note IS DISTINCT FROM 'alpha'"
    ).unwrap());
    assert_eq!(got, vec![2, 3, 4]);
}

#[test]
fn bare_column_is_not_distinct_from_non_null_literal_only_matches_equal() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE note IS NOT DISTINCT FROM 'beta'"
    ).unwrap());
    assert_eq!(got, vec![3]);
}

#[test]
fn bare_column_is_distinct_under_not() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // NOT (note IS DISTINCT FROM 'alpha') == note IS NOT DISTINCT FROM 'alpha'.
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE NOT (note IS DISTINCT FROM 'alpha')"
    ).unwrap());
    assert_eq!(got, vec![1]);
}

#[test]
fn bare_column_is_null_in_or_chain() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE note IS NULL OR note IS NOT DISTINCT FROM 'alpha'"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 4]);
}

#[test]
fn bare_column_other_matchers_still_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // LIKE on a bare column is not yet supported — still require JSON path.
    let err = db.execute(
        "SELECT id FROM t.public.items WHERE note LIKE 'a%'"
    ).unwrap_err();
    // Either the dispatcher stays on the PK path and complains
    // about a missing `=`, or the tree parser reports an
    // Unsupported matcher — both are correct rejections.
    assert!(matches!(err, SqlError::Unsupported(_) | SqlError::Syntax(_)), "got {err:?}");
}

#[test]
fn bare_column_is_distinct_rejects_type_mismatch() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Numeric literal against a TEXT column must fail type-check.
    let err = db.execute(
        "SELECT id FROM t.public.items WHERE note IS DISTINCT FROM 42"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)), "got {err:?}");
}

#[test]
fn bare_column_is_null_on_pk_is_never_true() {
    // PK is NOT NULL so this returns no rows.
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE id IS NULL"
    ).unwrap());
    assert!(got.is_empty());
}
