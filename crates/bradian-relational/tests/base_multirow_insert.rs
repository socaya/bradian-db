//! ADR 0220 — multi-row INSERT VALUES.

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::mvcc_sql::MvccSqlEngine;
use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn open(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    db
}

fn count(db: &SqlEngine) -> i64 {
    match db.execute("SELECT COUNT(*) FROM t.public.items").unwrap() {
        SqlResultSet::Rows { rows, .. } => match rows[0][0] {
            Value::Int64(n) => n,
            ref other => panic!("expected count; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn single_tuple_still_works() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    let affected = match db.execute(
        "INSERT INTO t.public.items(id, name) VALUES(1, 'alpha')"
    ).unwrap() {
        SqlResultSet::Command { affected } => affected,
        other => panic!("expected Command; got {other:?}"),
    };
    assert_eq!(affected, 1);
    assert_eq!(count(&db), 1);
}

#[test]
fn two_tuples() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    let affected = match db.execute(
        "INSERT INTO t.public.items(id, name) VALUES(1, 'alpha'), (2, 'bravo')"
    ).unwrap() {
        SqlResultSet::Command { affected } => affected,
        other => panic!("expected Command; got {other:?}"),
    };
    assert_eq!(affected, 2);
    assert_eq!(count(&db), 2);
}

#[test]
fn many_tuples() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    let affected = match db.execute(
        "INSERT INTO t.public.items(id, name) VALUES \
         (1, 'a'), (2, 'b'), (3, 'c'), (4, 'd'), (5, 'e')"
    ).unwrap() {
        SqlResultSet::Command { affected } => affected,
        other => panic!("expected Command; got {other:?}"),
    };
    assert_eq!(affected, 5);
    assert_eq!(count(&db), 5);
}

#[test]
fn second_tuple_bad_type_rejects_whole_batch() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    // Second tuple's `id` is a text literal — coercion should
    // fail before any row touches storage.
    let err = db.execute(
        "INSERT INTO t.public.items(id, name) VALUES(1, 'a'), ('oops', 'b')"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
    assert_eq!(count(&db), 0);
}

#[test]
fn mismatched_tuple_width_rejected() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    let err = db.execute(
        "INSERT INTO t.public.items(id, name) VALUES(1, 'a'), (2)"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
    assert_eq!(count(&db), 0);
}

#[test]
fn unique_violation_within_batch_rejects_second_row() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, label TEXT NOT NULL, UNIQUE(label), PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    // Second tuple duplicates `label` from the first inside
    // the same INSERT; UNIQUE must see the earlier batch row.
    assert!(db.execute(
        "INSERT INTO t.public.items(id, label) VALUES(1, 'x'), (2, 'x')"
    ).is_err());
    // The first row makes it to storage before the second is
    // rejected (non-atomic single-table engine).
    match db.execute("SELECT COUNT(*) FROM t.public.items").unwrap() {
        SqlResultSet::Rows { rows, .. } => {
            if let Value::Int64(n) = rows[0][0] { assert_eq!(n, 1); }
        }
        _ => panic!("expected count rows"),
    }
}

#[test]
fn pk_collision_within_batch_is_rejected() {
    let dir = tempdir().unwrap();
    let db = open(dir.path());
    // Same `id` twice — the second write is rejected by the PK
    // index (the exact SqlError variant depends on the store).
    assert!(db.execute(
        "INSERT INTO t.public.items(id, name) VALUES(1, 'a'), (1, 'b')"
    ).is_err());
}

#[test]
fn check_constraint_rejects_bad_row() {
    // The inline CHECK grammar at CREATE TABLE expects a scalar
    // comparison (col op lit). Column-op-column is reserved for
    // a future ADR; verify CHECK rejects a bad row using the
    // supported form (qty <> 0, batch with qty = 0 in tuple 2).
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, qty BIGINT NOT NULL CHECK (<> 0), PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    assert!(db.execute(
        "INSERT INTO t.public.items(id, qty) VALUES(1, 10), (2, 0), (3, 20)"
    ).is_err());
}

#[test]
fn domain_engine_multi_row() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.items").unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.items(id, name) VALUES(1, 'a'), (2, 'b'), (3, 'c')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(0i64));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } =
            s.execute("SELECT COUNT(*) FROM t.public.items")?
        {
            if let Value::Int64(n) = rows[0][0] { *captured.lock().unwrap() = n; }
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), 3);
}

#[test]
fn mvcc_engine_multi_row() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(dir.path()).unwrap();
        dom.register_catalog_table("t.public.items").unwrap();
    }
    let engine = MvccSqlEngine::open(dir.path()).unwrap();
    let affected = std::sync::Arc::new(std::sync::Mutex::new(0u64));
    let captured = affected.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Command { affected: n } =
            s.execute("INSERT INTO t.public.items(id, name) VALUES(1, 'a'), (2, 'b'), (3, 'c'), (4, 'd')")?
        {
            *captured.lock().unwrap() = n;
        }
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    assert_eq!(*affected.lock().unwrap(), 4);
}
