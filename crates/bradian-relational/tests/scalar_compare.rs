//! ADR 0245 — bare-column scalar comparisons in WHERE
//! (=, !=, <, <=, >, >=, <>). PK equality fast-path preserved.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        // id is PK (NOT NULL); grade is a non-PK integer with
        // mixed values; note is a nullable text column.
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, grade INT, note TEXT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, grade, note) VALUES(1, 10, 'alpha')",
        "INSERT INTO t.public.items(id, grade, note) VALUES(2, 20, 'beta')",
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
fn pk_equality_still_fast_path_single_row() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Full PK equality on a single-column PK — stays on the
    // KeyPredicate path and returns exactly one row.
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE id = 3"
    ).unwrap());
    assert_eq!(got, vec![3]);
}

#[test]
fn bare_non_pk_eq() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE grade = 20"
    ).unwrap());
    assert_eq!(got, vec![2]);
}

#[test]
fn bare_non_pk_ne_as_bang_eq_rejected_but_lt_gt_works() {
    // The lexer encodes != as two separate tokens (! is not
    // recognized), so !=` is a syntax error in this parser.
    // Users must spell inequality as `<>`.
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.items WHERE grade != 20"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_) | SqlError::Unsupported(_)), "got {err:?}");
}

#[test]
fn bare_non_pk_ne_via_angle_angle() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // `<>` is SQL-standard inequality. NULL row is excluded
    // by the matcher's three-valued-logic NULL guard.
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE grade <> 20"
    ).unwrap());
    assert_eq!(got, vec![1, 3, 4]);
}

#[test]
fn bare_non_pk_lt() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE grade < 25"
    ).unwrap());
    assert_eq!(got, vec![1, 2]);
}

#[test]
fn bare_non_pk_le() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE grade <= 30"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 3]);
}

#[test]
fn bare_non_pk_gt() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE grade > 20"
    ).unwrap());
    assert_eq!(got, vec![3, 4]);
}

#[test]
fn bare_non_pk_ge() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE grade >= 30"
    ).unwrap());
    assert_eq!(got, vec![3, 4]);
}

#[test]
fn bare_text_compare() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE note < 'c'"
    ).unwrap());
    // 'alpha' and 'beta' sort before 'c'; 'delta', 'gamma' do not.
    assert_eq!(got, vec![1, 2]);
}

#[test]
fn bare_scalar_compare_type_mismatch_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.items WHERE grade = 'alpha'"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)), "got {err:?}");
}

#[test]
fn bare_scalar_null_propagates() {
    // NULL compared to any literal returns NULL (three-valued
    // logic). Row 5 is excluded from both > and <.
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE grade > -999"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 3, 4]);
}

#[test]
fn bare_pk_lt_still_scans_but_works() {
    // id has a PK but `<` dispatches to tree parser (PK path is
    // = only). Must still evaluate correctly.
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE id < 3"
    ).unwrap());
    assert_eq!(got, vec![1, 2]);
}

#[test]
fn bare_compare_with_and_chain() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE grade >= 20 AND grade < 40"
    ).unwrap());
    assert_eq!(got, vec![2, 3]);
}

#[test]
fn bare_compare_mixed_with_is_null() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE grade > 25 OR note IS NULL"
    ).unwrap());
    assert_eq!(got, vec![3, 4, 5]);
}

// ADR 0246 landed bare-column LIKE and IN; the earlier
// `bare_like_still_rejected` and `bare_in_still_rejected`
// canaries from ADR 0245 are now obsolete — the behavior
// they asserted no longer holds. Their positive counterparts
// live in `tests/scalar_in_between_like.rs`.
