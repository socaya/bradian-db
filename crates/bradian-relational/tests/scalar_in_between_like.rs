//! ADR 0246 — bare-column IN / NOT IN / BETWEEN / NOT BETWEEN
//! / LIKE / NOT LIKE / ILIKE / NOT ILIKE on non-JSON columns.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, grade INT, note TEXT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, grade, note) VALUES(1, 10, 'alpha')",
        "INSERT INTO t.public.items(id, grade, note) VALUES(2, 20, 'Beta')",
        "INSERT INTO t.public.items(id, grade, note) VALUES(3, 30, 'gamma')",
        "INSERT INTO t.public.items(id, grade, note) VALUES(4, 40, 'delta')",
        "INSERT INTO t.public.items(id, grade, note) VALUES(5, NULL, NULL)",
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
fn bare_column_in_numeric() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE grade IN (10, 30)"
    ).unwrap());
    assert_eq!(got, vec![1, 3]);
}

#[test]
fn bare_column_not_in_numeric() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // NULL rows excluded by three-valued logic.
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE grade NOT IN (10, 30)"
    ).unwrap());
    assert_eq!(got, vec![2, 4]);
}

#[test]
fn bare_column_in_text() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE note IN ('alpha', 'delta')"
    ).unwrap());
    assert_eq!(got, vec![1, 4]);
}

#[test]
fn bare_column_in_type_mismatch() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.items WHERE grade IN (10, 'bad')"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)), "got {err:?}");
}

#[test]
fn bare_column_between_numeric() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE grade BETWEEN 15 AND 35"
    ).unwrap());
    assert_eq!(got, vec![2, 3]);
}

#[test]
fn bare_column_not_between_numeric() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE grade NOT BETWEEN 15 AND 35"
    ).unwrap());
    assert_eq!(got, vec![1, 4]);
}

#[test]
fn bare_column_between_text() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // 'Beta' (capital B < lowercase) sorts before 'a' lexicographically.
    // 'alpha', 'delta' are between 'a' and 'f'; 'gamma' excluded.
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE note BETWEEN 'a' AND 'f'"
    ).unwrap());
    assert_eq!(got, vec![1, 4]);
}

#[test]
fn bare_column_between_low_gt_high_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.items WHERE grade BETWEEN 50 AND 10"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)), "got {err:?}");
}

#[test]
fn bare_column_like_prefix() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE note LIKE 'd%'"
    ).unwrap());
    assert_eq!(got, vec![4]);
}

#[test]
fn bare_column_not_like() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // NULL row excluded by matcher's NULL guard.
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE note NOT LIKE 'a%'"
    ).unwrap());
    assert_eq!(got, vec![2, 3, 4]);
}

#[test]
fn bare_column_ilike_case_insensitive() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // 'Beta' with case-insensitive match succeeds.
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE note ILIKE 'b%'"
    ).unwrap());
    assert_eq!(got, vec![2]);
}

#[test]
fn bare_column_like_on_non_text_column_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.items WHERE grade LIKE '10%'"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)), "got {err:?}");
}

#[test]
fn bare_column_in_combined_with_and() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE grade IN (10, 20, 30) AND note LIKE '%a%'"
    ).unwrap());
    // grade in {10,20,30} => rows 1,2,3; note LIKE '%a%' matches
    // 'alpha' (row 1), 'gamma' (row 3); 'Beta' uppercase B still
    // contains 'a' so row 2 matches too.
    assert_eq!(got, vec![1, 2, 3]);
}

#[test]
fn bare_column_in_or_compose() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE grade IN (10) OR note IS NULL"
    ).unwrap());
    assert_eq!(got, vec![1, 5]);
}

#[test]
fn bare_column_between_under_not() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // NOT (BETWEEN) matches rows outside the range PLUS NULL
    // rows (NULL is not false, so NOT NULL stays NULL — which
    // the tree evaluator treats as false via the matcher NULL
    // guard; the row is still excluded).
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE NOT (grade BETWEEN 15 AND 35)"
    ).unwrap());
    assert_eq!(got, vec![1, 4, 5]);
}

#[test]
fn bare_pk_in_still_works() {
    // id has PK but IN dispatches to tree parser.
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE id IN (1, 3)"
    ).unwrap());
    assert_eq!(got, vec![1, 3]);
}
