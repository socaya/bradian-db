//! ADR 0227 — LEFT / RIGHT / FULL OUTER JOIN + table aliases.

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
        "INSERT INTO t.public.orders(id, user_id, total) VALUES(10, 1, 100), (11, 2, 50)",
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
fn left_outer_keeps_unmatched_left_rows() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT users.name, orders.total FROM t.public.users LEFT JOIN t.public.orders ON users.id = orders.user_id"
    ).unwrap());
    // Expected 3 rows: alice→100, bob→50, carol→NULL.
    assert_eq!(rows.len(), 3);
    let carol_row = rows.iter().find(|r| r[0] == Value::String("carol".into())).unwrap();
    assert_eq!(carol_row[1], Value::Null);
}

#[test]
fn right_outer_keeps_unmatched_right_rows() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.users (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.orders (id BIGINT NOT NULL, user_id BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.users(id) VALUES(1)",
        "INSERT INTO t.public.orders(id, user_id) VALUES(10, 1), (11, 99)",
    ] { db.execute(sql).unwrap(); }
    let (_, rows) = rows_of(db.execute(
        "SELECT users.id, orders.id FROM t.public.users RIGHT JOIN t.public.orders ON users.id = orders.user_id"
    ).unwrap());
    // Both orders kept; orphan one has NULL left.
    assert_eq!(rows.len(), 2);
    let orphan = rows.iter().find(|r| r[1] == Value::Int64(11)).unwrap();
    assert_eq!(orphan[0], Value::Null);
}

#[test]
fn full_outer_keeps_both_sides() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.a (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.b (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.a(id) VALUES(1), (2)",
        "INSERT INTO t.public.b(id) VALUES(2), (3)",
    ] { db.execute(sql).unwrap(); }
    let (_, rows) = rows_of(db.execute(
        "SELECT a.id, b.id FROM t.public.a FULL OUTER JOIN t.public.b ON a.id = b.id"
    ).unwrap());
    // Three rows: (1, NULL), (2, 2), (NULL, 3).
    assert_eq!(rows.len(), 3);
    assert!(rows.contains(&vec![Value::Int64(1), Value::Null]));
    assert!(rows.contains(&vec![Value::Int64(2), Value::Int64(2)]));
    assert!(rows.contains(&vec![Value::Null, Value::Int64(3)]));
}

#[test]
fn left_outer_with_optional_outer_keyword() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows_a) = rows_of(db.execute(
        "SELECT users.name FROM t.public.users LEFT JOIN t.public.orders ON users.id = orders.user_id"
    ).unwrap());
    let (_, rows_b) = rows_of(db.execute(
        "SELECT users.name FROM t.public.users LEFT OUTER JOIN t.public.orders ON users.id = orders.user_id"
    ).unwrap());
    assert_eq!(rows_a, rows_b);
}

#[test]
fn table_alias_in_projection_and_on() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (cols, rows) = rows_of(db.execute(
        "SELECT u.name, o.total FROM t.public.users AS u JOIN t.public.orders AS o ON u.id = o.user_id"
    ).unwrap());
    assert_eq!(cols, vec!["u.name", "o.total"]);
    assert_eq!(rows.len(), 2);
}

#[test]
fn table_alias_without_as_keyword() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (cols, rows) = rows_of(db.execute(
        "SELECT u.name, o.total FROM t.public.users u JOIN t.public.orders o ON u.id = o.user_id"
    ).unwrap());
    assert_eq!(cols, vec!["u.name", "o.total"]);
    assert_eq!(rows.len(), 2);
}

#[test]
fn inner_keyword_explicit() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT users.name, orders.total FROM t.public.users INNER JOIN t.public.orders ON users.id = orders.user_id"
    ).unwrap());
    assert_eq!(rows.len(), 2);
}

#[test]
fn left_outer_empty_right_keeps_all_left() {
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
        "SELECT users.id, orders.id FROM t.public.users LEFT JOIN t.public.orders ON users.id = orders.user_id"
    ).unwrap());
    assert_eq!(rows.len(), 2);
    for r in &rows { assert_eq!(r[1], Value::Null); }
}

#[test]
fn outer_join_with_where_on_right_filters_orphans() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // WHERE orders.total = 100 on a LEFT JOIN: orphans have
    // right = NULL, so WHERE right = 100 drops them.
    let (_, rows) = rows_of(db.execute(
        "SELECT users.name, orders.total FROM t.public.users LEFT JOIN t.public.orders ON users.id = orders.user_id WHERE orders.total = 100"
    ).unwrap());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::String("alice".into()));
}

#[test]
fn alias_resolution_rejects_bare_table_name_when_alias_present() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // When alias is bound, referring to the raw table name must fail.
    let err = db.execute(
        "SELECT users.name FROM t.public.users AS u JOIN t.public.orders AS o ON u.id = o.user_id"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Resolution(_)));
}
