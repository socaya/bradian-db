//! ADR 0176 — table-level `UNIQUE (col, ...)` at CREATE TABLE and
//! enforcement at INSERT in `SqlEngine`.
//!
//! Covers:
//! - Parser accepts `UNIQUE (col, ...)` clauses.
//! - First-insert of a unique value succeeds.
//! - Second insert with the same unique value is rejected with
//!   `SqlError::Unique`.
//! - Composite UNIQUE: only the full-tuple duplicate is rejected.
//! - SQL-standard NULL treatment: NULL in a unique column disables
//!   that constraint for the row.
//! - A table without UNIQUE still inserts duplicates freely (no
//!   accidental enforcement).
//! - UNIQUE with zero columns is rejected at parse time.

use bradian_relational::sql::{SqlEngine, SqlError};
use tempfile::tempdir;

fn open(dir: &std::path::Path) -> SqlEngine { SqlEngine::open(dir).unwrap() }

fn seed_unique_table(db: &SqlEngine) {
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.u (id BIGINT NOT NULL, email TEXT NOT NULL, UNIQUE (email), PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
}

#[test]
fn parser_accepts_unique_clause_and_first_insert_succeeds() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seed_unique_table(&db);
    db.execute("INSERT INTO t.public.u(id, email) VALUES(1, 'a@x')").unwrap();
}

#[test]
fn duplicate_unique_insert_is_rejected_with_sql_error_unique() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seed_unique_table(&db);
    db.execute("INSERT INTO t.public.u(id, email) VALUES(1, 'a@x')").unwrap();
    let err = db.execute(
        "INSERT INTO t.public.u(id, email) VALUES(2, 'a@x')",
    ).unwrap_err();
    match err {
        SqlError::Unique(v) => {
            assert_eq!(v.constraint, "uq_email");
            assert_eq!(v.columns, vec!["email".to_string()]);
        }
        other => panic!("expected SqlError::Unique; got {other:?}"),
    }
}

#[test]
fn distinct_unique_values_round_trip_freely() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    seed_unique_table(&db);
    for (i, email) in [(1, "a@x"), (2, "b@x"), (3, "c@x")] {
        db.execute(&format!(
            "INSERT INTO t.public.u(id, email) VALUES({i}, '{email}')"
        )).unwrap();
    }
}

#[test]
fn composite_unique_rejects_only_full_tuple_duplicate() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.c (id BIGINT NOT NULL, first TEXT NOT NULL, last TEXT NOT NULL, UNIQUE (first, last), PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    db.execute("INSERT INTO t.public.c(id, first, last) VALUES(1, 'alice', 'A')").unwrap();
    // Same first, different last — allowed.
    db.execute("INSERT INTO t.public.c(id, first, last) VALUES(2, 'alice', 'B')").unwrap();
    // Same last, different first — allowed.
    db.execute("INSERT INTO t.public.c(id, first, last) VALUES(3, 'bob', 'A')").unwrap();
    // Full-tuple duplicate — rejected.
    let err = db.execute(
        "INSERT INTO t.public.c(id, first, last) VALUES(4, 'alice', 'A')",
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unique(_)));
}

#[test]
fn null_in_unique_column_disables_the_constraint_for_that_row() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.n (id BIGINT NOT NULL, email TEXT, UNIQUE (email), PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    db.execute("INSERT INTO t.public.n(id, email) VALUES(1, NULL)").unwrap();
    // A second NULL does not collide — SQL-standard semantics.
    db.execute("INSERT INTO t.public.n(id, email) VALUES(2, NULL)").unwrap();
    // A non-NULL still enforces.
    db.execute("INSERT INTO t.public.n(id, email) VALUES(3, 'x@y')").unwrap();
    let err = db.execute(
        "INSERT INTO t.public.n(id, email) VALUES(4, 'x@y')",
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unique(_)));
}

#[test]
fn table_without_unique_accepts_duplicates_freely() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.p (id BIGINT NOT NULL, email TEXT NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    db.execute("INSERT INTO t.public.p(id, email) VALUES(1, 'a@x')").unwrap();
    // No UNIQUE → duplicates allowed.
    db.execute("INSERT INTO t.public.p(id, email) VALUES(2, 'a@x')").unwrap();
}

#[test]
fn unique_with_no_columns_is_rejected_at_parse() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
    ] { db.execute(sql).unwrap(); }
    let err = db.execute(
        "CREATE TABLE t.public.z (id BIGINT NOT NULL, UNIQUE (), PRIMARY KEY(id))"
    ).unwrap_err();
    // Either Syntax (empty list is malformed) or Resolution
    // (p.list with no elements). We accept either path.
    assert!(matches!(err, SqlError::Syntax(_) | SqlError::Resolution(_) | SqlError::Unsupported(_)),
        "empty UNIQUE must fail cleanly; got {err:?}");
}

#[test]
fn unique_constraint_persists_after_restart() {
    let dir = tempdir().unwrap();
    {
        let db = open(dir.path());
        seed_unique_table(&db);
        db.execute("INSERT INTO t.public.u(id, email) VALUES(1, 'a@x')").unwrap();
    }
    // Reopen; constraint must still block the duplicate.
    let db = open(dir.path());
    let err = db.execute(
        "INSERT INTO t.public.u(id, email) VALUES(2, 'a@x')",
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unique(_)));
}
