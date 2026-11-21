//! ADR 0182 — minimal column-level CHECK constraint.
//!
//! Scope:
//! - Parse `CHECK (<op> <literal>)` after optional NOT NULL / COLLATE.
//! - Supported ops: =, <>, <, <=, >, >=.
//! - Evaluate at INSERT and UPDATE in every engine (SqlEngine covered
//!   directly; other engines share the same `check_row` helper).
//! - NULL in a checked column skips the CHECK (SQL three-valued).

use bradian_relational::sql::{SqlEngine, SqlError};
use tempfile::tempdir;

fn open(dir: &std::path::Path) -> SqlEngine { SqlEngine::open(dir).unwrap() }

fn seed_qty_positive(db: &SqlEngine) {
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, qty INT NOT NULL CHECK (> 0), PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
}

#[test]
fn check_greater_than_zero_accepts_positive_and_rejects_zero_or_negative() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seed_qty_positive(&db);
    db.execute("INSERT INTO t.public.items(id, qty) VALUES(1, 5)").unwrap();
    let err = db.execute(
        "INSERT INTO t.public.items(id, qty) VALUES(2, 0)",
    ).unwrap_err();
    match err {
        SqlError::Check { column, op, .. } => {
            assert_eq!(column, "qty");
            assert_eq!(op, ">");
        }
        other => panic!("expected SqlError::Check; got {other:?}"),
    }
    let err = db.execute(
        "INSERT INTO t.public.items(id, qty) VALUES(3, -1)",
    ).unwrap_err();
    assert!(matches!(err, SqlError::Check { .. }));
}

#[test]
fn check_update_mutated_row_is_rejected_when_predicate_fails() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seed_qty_positive(&db);
    db.execute("INSERT INTO t.public.items(id, qty) VALUES(1, 5)").unwrap();
    let err = db.execute(
        "UPDATE t.public.items SET qty=0 WHERE id=1",
    ).unwrap_err();
    assert!(matches!(err, SqlError::Check { column, .. } if column == "qty"));
}

#[test]
fn multiple_check_clauses_on_one_column_are_all_enforced() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, qty INT NOT NULL CHECK (> 0) CHECK (< 100), PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    db.execute("INSERT INTO t.public.items(id, qty) VALUES(1, 50)").unwrap();
    // Fails second check only.
    assert!(matches!(
        db.execute("INSERT INTO t.public.items(id, qty) VALUES(2, 100)").unwrap_err(),
        SqlError::Check { .. }
    ));
    // Fails first check only.
    assert!(matches!(
        db.execute("INSERT INTO t.public.items(id, qty) VALUES(3, 0)").unwrap_err(),
        SqlError::Check { .. }
    ));
}

#[test]
fn check_null_skips_the_predicate_per_sql_three_valued_logic() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, qty INT CHECK (> 0), PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    // NULL qty — CHECK (> 0) is UNKNOWN, treated as not-false.
    db.execute("INSERT INTO t.public.items(id, qty) VALUES(1, NULL)").unwrap();
}

#[test]
fn all_six_operators_bind_correctly() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.eq (id BIGINT NOT NULL, v INT NOT NULL CHECK (= 7), PRIMARY KEY(id))",
        "CREATE TABLE t.public.ne (id BIGINT NOT NULL, v INT NOT NULL CHECK (<> 0), PRIMARY KEY(id))",
        "CREATE TABLE t.public.lt (id BIGINT NOT NULL, v INT NOT NULL CHECK (< 10), PRIMARY KEY(id))",
        "CREATE TABLE t.public.le (id BIGINT NOT NULL, v INT NOT NULL CHECK (<= 10), PRIMARY KEY(id))",
        "CREATE TABLE t.public.gt (id BIGINT NOT NULL, v INT NOT NULL CHECK (> 0), PRIMARY KEY(id))",
        "CREATE TABLE t.public.ge (id BIGINT NOT NULL, v INT NOT NULL CHECK (>= 0), PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    // Satisfying values.
    db.execute("INSERT INTO t.public.eq(id, v) VALUES(1, 7)").unwrap();
    db.execute("INSERT INTO t.public.ne(id, v) VALUES(1, 5)").unwrap();
    db.execute("INSERT INTO t.public.lt(id, v) VALUES(1, 9)").unwrap();
    db.execute("INSERT INTO t.public.le(id, v) VALUES(1, 10)").unwrap();
    db.execute("INSERT INTO t.public.gt(id, v) VALUES(1, 1)").unwrap();
    db.execute("INSERT INTO t.public.ge(id, v) VALUES(1, 0)").unwrap();
    // Just-failing values.
    for (t, v) in [("eq", 8), ("ne", 0), ("lt", 10), ("le", 11), ("gt", 0), ("ge", -1)] {
        let err = db.execute(&format!("INSERT INTO t.public.{t}(id, v) VALUES(2, {v})")).unwrap_err();
        assert!(matches!(err, SqlError::Check { .. }),
            "table {t} v={v} should fail CHECK; got {err:?}");
    }
}

#[test]
fn check_literal_must_match_column_type_at_create() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
    ] { db.execute(sql).unwrap(); }
    // CHECK (> 'abc') on BIGINT column — literal is TEXT, type
    // mismatch caught at CREATE (SqlError::Type during coerce).
    let err = db.execute(
        "CREATE TABLE t.public.a (id BIGINT NOT NULL CHECK (> 'abc'), PRIMARY KEY(id))"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)),
        "mismatched CHECK literal type should fail at CREATE; got {err:?}");
}

#[test]
fn check_persists_across_restart() {
    let dir = tempdir().unwrap();
    {
        let db = open(dir.path());
        seed_qty_positive(&db);
        db.execute("INSERT INTO t.public.items(id, qty) VALUES(1, 5)").unwrap();
    }
    // Reopen — CHECK must still reject qty=0.
    let db = open(dir.path());
    let err = db.execute(
        "INSERT INTO t.public.items(id, qty) VALUES(2, 0)",
    ).unwrap_err();
    assert!(matches!(err, SqlError::Check { .. }));
}
