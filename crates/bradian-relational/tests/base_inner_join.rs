//! ADR 0226 — simple INNER JOIN.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.users (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.orders (id BIGINT NOT NULL, user_id BIGINT NOT NULL, total BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.users(id, name) VALUES(1, 'alice'), (2, 'bob'), (3, 'carol')",
        "INSERT INTO t.public.orders(id, user_id, total) VALUES(10, 1, 100), (11, 1, 200), (12, 2, 50)",
    ] { db.execute(sql).unwrap(); }
    db
}

fn rows_of(rs: SqlResultSet) -> (Vec<String>, Vec<Vec<Value>>) {
    match rs {
        SqlResultSet::Rows { columns, rows, .. } => (columns, rows),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn inner_join_basic() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (cols, rows) = rows_of(db.execute(
        "SELECT users.name, orders.total FROM t.public.users JOIN t.public.orders ON users.id = orders.user_id"
    ).unwrap());
    assert_eq!(cols, vec!["users.name", "orders.total"]);
    // alice has 2 orders, bob has 1, carol has 0 → 3 rows total.
    assert_eq!(rows.len(), 3);
}

#[test]
fn inner_join_rejects_bare_projection() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT name, total FROM t.public.users JOIN t.public.orders ON users.id = orders.user_id"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn inner_join_empty_right() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.users (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.orders (id BIGINT NOT NULL, user_id BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.users(id) VALUES(1), (2)",
    ] { db.execute(sql).unwrap(); }
    let (_, rows) = rows_of(db.execute(
        "SELECT users.id, orders.id FROM t.public.users JOIN t.public.orders ON users.id = orders.user_id"
    ).unwrap());
    assert!(rows.is_empty());
}

#[test]
fn inner_join_with_where() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT users.name, orders.total FROM t.public.users JOIN t.public.orders ON users.id = orders.user_id WHERE users.id = 1"
    ).unwrap());
    // alice has 2 orders.
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], Value::String("alice".into()));
}

#[test]
fn inner_join_where_on_right_side() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT users.name, orders.total FROM t.public.users JOIN t.public.orders ON users.id = orders.user_id WHERE orders.total = 50"
    ).unwrap());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::String("bob".into()));
}

#[test]
fn inner_join_on_order_reversed() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Swap left/right in ON: orders.user_id = users.id.
    let (_, rows) = rows_of(db.execute(
        "SELECT users.name, orders.total FROM t.public.users JOIN t.public.orders ON orders.user_id = users.id"
    ).unwrap());
    assert_eq!(rows.len(), 3);
}

#[test]
fn inner_join_projection_includes_only_left() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (cols, rows) = rows_of(db.execute(
        "SELECT users.name FROM t.public.users JOIN t.public.orders ON users.id = orders.user_id"
    ).unwrap());
    assert_eq!(cols, vec!["users.name"]);
    assert_eq!(rows.len(), 3);
}

#[test]
fn inner_join_unknown_table_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT users.name, missing.total FROM t.public.users JOIN t.public.orders ON users.id = orders.user_id"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Resolution(_)));
}

#[test]
fn inner_join_wildcard_concatenates_schemas() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (cols, rows) = rows_of(db.execute(
        "SELECT * FROM t.public.users JOIN t.public.orders ON users.id = orders.user_id"
    ).unwrap());
    // users has (id, name), orders has (id, user_id, total) → 5 columns.
    assert_eq!(cols.len(), 5);
    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert_eq!(row.len(), 5);
    }
}

#[test]
fn inner_join_no_matches_returns_empty() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.users (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.orders (id BIGINT NOT NULL, user_id BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.users(id) VALUES(1)",
        "INSERT INTO t.public.orders(id, user_id) VALUES(10, 99)",
    ] { db.execute(sql).unwrap(); }
    let (_, rows) = rows_of(db.execute(
        "SELECT users.id, orders.id FROM t.public.users JOIN t.public.orders ON users.id = orders.user_id"
    ).unwrap());
    assert!(rows.is_empty());
}

#[test]
fn dotted_projection_rejected_without_join() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT users.name FROM t.public.users"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}
