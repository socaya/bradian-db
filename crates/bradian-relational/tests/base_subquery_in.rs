//! ADR 0229 — WHERE col IN (SELECT …) subquery.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.users (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.orders (id BIGINT NOT NULL, user_id BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.users(id, name) VALUES(1, 'alice'), (2, 'bob'), (3, 'carol')",
        "INSERT INTO t.public.orders(id, user_id) VALUES(10, 1), (11, 2)",
    ] { db.execute(sql).unwrap(); }
    db
}

fn rows_of(rs: SqlResultSet) -> Vec<Vec<Value>> {
    match rs {
        SqlResultSet::Rows { rows, .. } => rows,
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn in_subquery_selects_rows_matching_result() {
    let dir = tempdir().unwrap();
    let _seed = seed(dir.path());
    // Use a dedicated DB so IN (SELECT …) resolution keeps a
    // JSON-column WHERE on the outer query (the only non-PK
    // WHERE shape the base engine admits).
    let db = {
        let db = SqlEngine::open(tempdir().unwrap().path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
            "CREATE TABLE t.public.keep (id BIGINT NOT NULL, PRIMARY KEY(id))",
            r#"INSERT INTO t.public.items(id, body) VALUES(1, '{"k":1}'), (2, '{"k":2}'), (3, '{"k":3}')"#,
            "INSERT INTO t.public.keep(id) VALUES(2), (3)",
        ] { db.execute(sql).unwrap(); }
        db
    };
    let got = rows_of(db.execute(
        "SELECT id FROM t.public.items WHERE body -> 'k' IN (SELECT id FROM t.public.keep)"
    ).unwrap());
    assert_eq!(got, vec![vec![Value::Int64(2)], vec![Value::Int64(3)]]);
    // Suppress unused-var warning for `seed` fixture when
    // only the inner `db` is used.
    let _ = seed;
}

#[test]
fn not_in_subquery_excludes_rows() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.drop_ids (id BIGINT NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.items(id, body) VALUES(1, '{"k":1}'), (2, '{"k":2}'), (3, '{"k":3}')"#,
        "INSERT INTO t.public.drop_ids(id) VALUES(2)",
    ] { db.execute(sql).unwrap(); }
    let got = rows_of(db.execute(
        "SELECT id FROM t.public.items WHERE body -> 'k' NOT IN (SELECT id FROM t.public.drop_ids)"
    ).unwrap());
    assert_eq!(got, vec![vec![Value::Int64(1)], vec![Value::Int64(3)]]);
}

#[test]
fn empty_subquery_leaves_no_matches() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.keep (id BIGINT NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.items(id, body) VALUES(1, '{"k":1}')"#,
    ] { db.execute(sql).unwrap(); }
    let got = rows_of(db.execute(
        "SELECT id FROM t.public.items WHERE body -> 'k' IN (SELECT id FROM t.public.keep)"
    ).unwrap());
    assert!(got.is_empty());
}

#[test]
fn text_subquery_values() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.tags (id BIGINT NOT NULL, label TEXT NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.items(id, body) VALUES(1, '{"t":"a"}'), (2, '{"t":"b"}'), (3, '{"t":"c"}')"#,
        "INSERT INTO t.public.tags(id, label) VALUES(1, 'a'), (2, 'c')",
    ] { db.execute(sql).unwrap(); }
    let got = rows_of(db.execute(
        "SELECT id FROM t.public.items WHERE body ->> 't' IN (SELECT label FROM t.public.tags)"
    ).unwrap());
    assert_eq!(got, vec![vec![Value::Int64(1)], vec![Value::Int64(3)]]);
}

#[test]
fn subquery_with_where() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.keep (id BIGINT NOT NULL, flag BIGINT NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.items(id, body) VALUES(1, '{"k":1}'), (2, '{"k":2}'), (3, '{"k":3}')"#,
        "INSERT INTO t.public.keep(id, flag) VALUES(1, 1), (2, 0), (3, 1)",
    ] { db.execute(sql).unwrap(); }
    let got = rows_of(db.execute(
        "SELECT id FROM t.public.items WHERE body -> 'k' IN (SELECT id FROM t.public.keep WHERE id = 3)"
    ).unwrap());
    assert_eq!(got, vec![vec![Value::Int64(3)]]);
}

#[test]
fn subquery_multi_column_rejected() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.keep (id BIGINT NOT NULL, label TEXT NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.items(id, body) VALUES(1, '{"k":1}')"#,
        "INSERT INTO t.public.keep(id, label) VALUES(1, 'a')",
    ] { db.execute(sql).unwrap(); }
    let err = db.execute(
        "SELECT id FROM t.public.items WHERE body -> 'k' IN (SELECT id, label FROM t.public.keep)"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}
