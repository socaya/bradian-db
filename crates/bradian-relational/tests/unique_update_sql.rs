//! ADR 0177 — UNIQUE enforcement on `SqlEngine` UPDATE.
//!
//! Scenarios:
//! - Updating a non-unique column stays allowed (no collision).
//! - Updating a row's unique value to a brand-new value stays
//!   allowed.
//! - Updating a row's unique value to another row's current
//!   value is rejected with `SqlError::Unique`.
//! - Updating a row's unique column to its own current value is
//!   allowed (self-match is filtered by PK).
//! - Composite UNIQUE: swapping one component to collide with
//!   another row's tuple is rejected.
//! - WHERE id=<non-existent> matches zero rows and does not
//!   enforce anything.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use tempfile::tempdir;

fn open(dir: &std::path::Path) -> SqlEngine { SqlEngine::open(dir).unwrap() }

fn seeded_single_unique(db: &SqlEngine) {
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.u (id BIGINT NOT NULL, email TEXT NOT NULL, note TEXT NOT NULL, UNIQUE (email), PRIMARY KEY(id))",
        "INSERT INTO t.public.u(id, email, note) VALUES(1, 'a@x', 'one')",
        "INSERT INTO t.public.u(id, email, note) VALUES(2, 'b@x', 'two')",
        "INSERT INTO t.public.u(id, email, note) VALUES(3, 'c@x', 'three')",
    ] { db.execute(sql).unwrap(); }
}

#[test]
fn update_non_unique_column_is_always_allowed() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seeded_single_unique(&db);
    db.execute("UPDATE t.public.u SET note='updated' WHERE id=1").unwrap();
}

#[test]
fn update_unique_column_to_brand_new_value_is_allowed() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seeded_single_unique(&db);
    db.execute("UPDATE t.public.u SET email='z@x' WHERE id=1").unwrap();
    // Verify via SELECT.
    match db.execute("SELECT email FROM t.public.u WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            match &rows[0][0] {
                bradian_relational::typed_table::Value::String(s) => assert_eq!(s, "z@x"),
                other => panic!("expected String; got {other:?}"),
            }
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn update_unique_column_to_another_rows_current_value_is_rejected() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seeded_single_unique(&db);
    let err = db.execute(
        "UPDATE t.public.u SET email='b@x' WHERE id=1",
    ).unwrap_err();
    match err {
        SqlError::Unique(v) => {
            assert_eq!(v.constraint, "uq_email");
        }
        other => panic!("expected SqlError::Unique; got {other:?}"),
    }
}

#[test]
fn update_unique_column_to_its_own_current_value_is_allowed() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seeded_single_unique(&db);
    // No net change; the row is filtered out of the existing
    // iterator by PK so the "collision" with itself is a
    // non-event.
    db.execute("UPDATE t.public.u SET email='a@x' WHERE id=1").unwrap();
}

#[test]
fn update_composite_unique_partial_overlap_is_allowed() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.c (id BIGINT NOT NULL, first TEXT NOT NULL, last TEXT NOT NULL, note TEXT NOT NULL, UNIQUE (first, last), PRIMARY KEY(id))",
        "INSERT INTO t.public.c(id, first, last, note) VALUES(1, 'alice', 'A', 'x')",
        "INSERT INTO t.public.c(id, first, last, note) VALUES(2, 'bob', 'B', 'y')",
    ] { db.execute(sql).unwrap(); }
    // Change row 1's last to 'C' — tuple becomes ('alice','C'),
    // doesn't collide with ('bob','B').
    db.execute("UPDATE t.public.c SET last='C' WHERE id=1").unwrap();
}

#[test]
fn update_composite_unique_full_tuple_collision_is_rejected() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.c (id BIGINT NOT NULL, first TEXT NOT NULL, last TEXT NOT NULL, UNIQUE (first, last), PRIMARY KEY(id))",
        "INSERT INTO t.public.c(id, first, last) VALUES(1, 'alice', 'A')",
        "INSERT INTO t.public.c(id, first, last) VALUES(2, 'bob', 'B')",
    ] { db.execute(sql).unwrap(); }
    // UPDATE row 1 so its tuple becomes ('bob','B') —
    // collides with row 2.
    let err = db.execute(
        "UPDATE t.public.c SET first='bob', last='B' WHERE id=1",
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unique(_)));
}

#[test]
fn update_where_matches_nothing_is_a_noop_and_never_enforces() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seeded_single_unique(&db);
    // No row matches id=999 — even a would-be colliding value
    // is a no-op (affected=0) because the UPDATE never gets
    // to the checker.
    match db.execute("UPDATE t.public.u SET email='a@x' WHERE id=999").unwrap() {
        SqlResultSet::Command { affected } => assert_eq!(affected, 0),
        other => panic!("expected Command; got {other:?}"),
    }
}

#[test]
fn update_to_null_in_unique_nullable_column_does_not_collide() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.n (id BIGINT NOT NULL, email TEXT, UNIQUE (email), PRIMARY KEY(id))",
        "INSERT INTO t.public.n(id, email) VALUES(1, NULL)",
        "INSERT INTO t.public.n(id, email) VALUES(2, 'b@x')",
    ] { db.execute(sql).unwrap(); }
    // Row 2's email → NULL. SQL-standard: NULL disables the
    // constraint for that row; coexists with row 1's NULL.
    db.execute("UPDATE t.public.n SET email=NULL WHERE id=2").unwrap();
}
