//! ADR 0228 — aggregates + GROUP BY in joined queries.

use bradian_relational::sql::{SqlEngine, SqlResultSet};
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
        "INSERT INTO t.public.orders(id, user_id, total) VALUES(10, 1, 100), (11, 1, 200), (12, 2, 50), (13, 2, 70)",
    ] { db.execute(sql).unwrap(); }
    db
}

fn rows_of(rs: SqlResultSet) -> (Vec<String>, Vec<Vec<Value>>) {
    match rs {
        SqlResultSet::Rows { columns, rows } => (columns, rows),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn join_count_per_user() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (cols, rows) = rows_of(db.execute(
        "SELECT u.name, COUNT(*) FROM t.public.users AS u JOIN t.public.orders AS o ON u.id = o.user_id GROUP BY u.name"
    ).unwrap());
    assert_eq!(cols, vec!["u.name", "count"]);
    assert_eq!(rows, vec![
        vec![Value::String("alice".into()), Value::Int64(2)],
        vec![Value::String("bob".into()), Value::Int64(2)],
    ]);
}

#[test]
fn join_sum_per_user() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT u.name, SUM(o.total) FROM t.public.users AS u JOIN t.public.orders AS o ON u.id = o.user_id GROUP BY u.name"
    ).unwrap());
    assert_eq!(rows, vec![
        vec![Value::String("alice".into()), Value::Int64(300)],
        vec![Value::String("bob".into()), Value::Int64(120)],
    ]);
}

#[test]
fn join_min_max_per_user() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT u.name, MIN(o.total), MAX(o.total) FROM t.public.users AS u JOIN t.public.orders AS o ON u.id = o.user_id GROUP BY u.name"
    ).unwrap());
    assert_eq!(rows[0], vec![Value::String("alice".into()), Value::Int64(100), Value::Int64(200)]);
    assert_eq!(rows[1], vec![Value::String("bob".into()), Value::Int64(50), Value::Int64(70)]);
}

#[test]
fn join_scalar_count_star() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (cols, rows) = rows_of(db.execute(
        "SELECT COUNT(*) FROM t.public.users AS u JOIN t.public.orders AS o ON u.id = o.user_id"
    ).unwrap());
    assert_eq!(cols, vec!["count"]);
    assert_eq!(rows, vec![vec![Value::Int64(4)]]);
}

#[test]
fn join_scalar_sum() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT SUM(o.total) FROM t.public.users AS u JOIN t.public.orders AS o ON u.id = o.user_id"
    ).unwrap());
    assert_eq!(rows, vec![vec![Value::Int64(420)]]);
}

#[test]
fn join_having_filters_groups() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT u.name, SUM(o.total) FROM t.public.users AS u JOIN t.public.orders AS o ON u.id = o.user_id GROUP BY u.name HAVING SUM(o.total) >= 200"
    ).unwrap());
    assert_eq!(rows, vec![
        vec![Value::String("alice".into()), Value::Int64(300)],
    ]);
}

#[test]
fn left_join_count_includes_zero_rows() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // ADR 0228 — carol has no orders. LEFT JOIN emits a
    // (carol, NULL) composite; COUNT(o.id) skips NULL and
    // yields 0 for carol's bucket. COUNT(*) counts the
    // composite row so carol's bucket is 1.
    let (_, rows) = rows_of(db.execute(
        "SELECT u.name, COUNT(o.id) FROM t.public.users AS u LEFT JOIN t.public.orders AS o ON u.id = o.user_id GROUP BY u.name"
    ).unwrap());
    let carol = rows.iter().find(|r| r[0] == Value::String("carol".into())).unwrap();
    assert_eq!(carol[1], Value::Int64(0));
}

#[test]
fn join_group_by_two_cols() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT u.name, o.user_id, COUNT(*) FROM t.public.users AS u JOIN t.public.orders AS o ON u.id = o.user_id GROUP BY u.name, o.user_id"
    ).unwrap());
    assert_eq!(rows.len(), 2);
}

#[test]
fn join_count_distinct_per_user() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.users (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.orders (id BIGINT NOT NULL, user_id BIGINT NOT NULL, product TEXT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.users(id) VALUES(1), (2)",
        "INSERT INTO t.public.orders(id, user_id, product) VALUES(10, 1, 'a'), (11, 1, 'a'), (12, 1, 'b'), (13, 2, 'a')",
    ] { db.execute(sql).unwrap(); }
    let (_, rows) = rows_of(db.execute(
        "SELECT u.id, COUNT(DISTINCT o.product) FROM t.public.users AS u JOIN t.public.orders AS o ON u.id = o.user_id GROUP BY u.id"
    ).unwrap());
    assert_eq!(rows[0], vec![Value::Int64(1), Value::Int64(2)]);
    assert_eq!(rows[1], vec![Value::Int64(2), Value::Int64(1)]);
}
