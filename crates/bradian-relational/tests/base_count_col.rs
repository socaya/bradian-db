//! ADR 0216 — COUNT(col) and COUNT(DISTINCT col).

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::mvcc_sql::MvccSqlEngine;
use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, kind TEXT, qty BIGINT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, kind, qty) VALUES(1, 'a', 10)",
        "INSERT INTO t.public.items(id, kind, qty) VALUES(2, 'a', NULL)",
        "INSERT INTO t.public.items(id, kind, qty) VALUES(3, 'b', 10)",
        "INSERT INTO t.public.items(id, kind, qty) VALUES(4, NULL, 20)",
        "INSERT INTO t.public.items(id, kind, qty) VALUES(5, 'b', 30)",
        "INSERT INTO t.public.items(id, kind, qty) VALUES(6, NULL, NULL)",
    ] { db.execute(sql).unwrap(); }
    db
}

fn single_count(rs: SqlResultSet) -> i64 {
    match rs {
        SqlResultSet::Rows { columns, rows } => {
            assert_eq!(columns, vec!["count"]);
            assert_eq!(rows.len(), 1);
            match rows[0][0] {
                Value::Int64(n) => n,
                ref other => panic!("expected Int64; got {other:?}"),
            }
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn count_of_nullable_text_skips_nulls() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Rows with non-null kind: 1,2,3,5 → 4.
    assert_eq!(
        single_count(db.execute("SELECT COUNT(kind) FROM t.public.items").unwrap()),
        4,
    );
}

#[test]
fn count_of_nullable_numeric_skips_nulls() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Rows with non-null qty: 1,3,4,5 → 4.
    assert_eq!(
        single_count(db.execute("SELECT COUNT(qty) FROM t.public.items").unwrap()),
        4,
    );
}

#[test]
fn count_of_not_null_column_matches_count_star() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        single_count(db.execute("SELECT COUNT(id) FROM t.public.items").unwrap()),
        6,
    );
    assert_eq!(
        single_count(db.execute("SELECT COUNT(*) FROM t.public.items").unwrap()),
        6,
    );
}

#[test]
fn count_distinct_text() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Distinct non-null kind: a, b → 2.
    assert_eq!(
        single_count(db.execute("SELECT COUNT(DISTINCT kind) FROM t.public.items").unwrap()),
        2,
    );
}

#[test]
fn count_distinct_numeric() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Distinct non-null qty: 10, 20, 30 → 3.
    assert_eq!(
        single_count(db.execute("SELECT COUNT(DISTINCT qty) FROM t.public.items").unwrap()),
        3,
    );
}

#[test]
fn count_distinct_empty_table() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, kind TEXT, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    assert_eq!(
        single_count(db.execute("SELECT COUNT(DISTINCT kind) FROM t.public.items").unwrap()),
        0,
    );
}

#[test]
fn count_distinct_all_null() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, kind TEXT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, kind) VALUES(1, NULL)",
        "INSERT INTO t.public.items(id, kind) VALUES(2, NULL)",
    ] { db.execute(sql).unwrap(); }
    assert_eq!(
        single_count(db.execute("SELECT COUNT(DISTINCT kind) FROM t.public.items").unwrap()),
        0,
    );
}

#[test]
fn count_col_with_pk_where() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        single_count(db.execute("SELECT COUNT(kind) FROM t.public.items WHERE id = 4").unwrap()),
        0, // id=4 has kind NULL
    );
    assert_eq!(
        single_count(db.execute("SELECT COUNT(kind) FROM t.public.items WHERE id = 1").unwrap()),
        1,
    );
}

#[test]
fn count_distinct_with_where() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Only row id=5 → one distinct kind value.
    assert_eq!(
        single_count(db.execute("SELECT COUNT(DISTINCT kind) FROM t.public.items WHERE id = 5").unwrap()),
        1,
    );
}

#[test]
fn count_unknown_column_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute("SELECT COUNT(nope) FROM t.public.items").unwrap_err();
    assert!(matches!(err, SqlError::Resolution(_)));
}

#[test]
fn count_distinct_unknown_column_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute("SELECT COUNT(DISTINCT nope) FROM t.public.items").unwrap_err();
    assert!(matches!(err, SqlError::Resolution(_)));
}

#[test]
fn domain_engine_count_distinct() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, k TEXT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.items").unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.items(id, k) VALUES(1, 'x')")?;
        s.execute("INSERT INTO t.public.items(id, k) VALUES(2, 'x')")?;
        s.execute("INSERT INTO t.public.items(id, k) VALUES(3, 'y')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(0i64));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } =
            s.execute("SELECT COUNT(DISTINCT k) FROM t.public.items")?
        {
            if let Value::Int64(n) = rows[0][0] { *captured.lock().unwrap() = n; }
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), 2);
}

#[test]
fn mvcc_engine_count_col() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, note TEXT, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(dir.path()).unwrap();
        dom.register_catalog_table("t.public.items").unwrap();
    }
    let engine = MvccSqlEngine::open(dir.path()).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.items(id, note) VALUES(1, 'hi')")?;
        s.execute("INSERT INTO t.public.items(id, note) VALUES(2, NULL)")?;
        s.execute("INSERT INTO t.public.items(id, note) VALUES(3, 'bye')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(0i64));
    let captured = got.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } =
            s.execute("SELECT COUNT(note) FROM t.public.items")?
        {
            if let Value::Int64(n) = rows[0][0] { *captured.lock().unwrap() = n; }
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), 2);
}
