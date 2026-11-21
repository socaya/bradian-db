//! ADR 0179 — UNIQUE enforcement in `MvccSqlEngine` INSERT + UPDATE.

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::mvcc_sql::MvccSqlEngine;
use bradian_relational::sql::{SqlEngine, SqlError};
use tempfile::tempdir;

fn setup(root: &std::path::Path) {
    {
        let ddl = SqlEngine::open(root).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.u (id BIGINT NOT NULL, email TEXT NOT NULL, note TEXT NOT NULL, UNIQUE (email), PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(root).unwrap();
        dom.register_catalog_table("t.public.u").unwrap();
    }
}

#[test]
fn mvcc_duplicate_insert_is_rejected() {
    let base = tempdir().unwrap();
    setup(base.path());
    let engine = MvccSqlEngine::open(base.path()).unwrap();
    let err = engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.u(id, email, note) VALUES(1, 'a@x', 'one')")?;
        s.execute("INSERT INTO t.public.u(id, email, note) VALUES(2, 'a@x', 'two')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap_err();
    assert!(matches!(err, SqlError::Unique(_)),
        "expected SqlError::Unique; got {err:?}");
}

#[test]
fn mvcc_update_collision_is_rejected() {
    let base = tempdir().unwrap();
    setup(base.path());
    let engine = MvccSqlEngine::open(base.path()).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.u(id, email, note) VALUES(1, 'a@x', 'one')")?;
        s.execute("INSERT INTO t.public.u(id, email, note) VALUES(2, 'b@x', 'two')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let err = engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE t.public.u SET email='b@x' WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap_err();
    assert!(matches!(err, SqlError::Unique(_)));
}

#[test]
fn mvcc_self_update_is_allowed() {
    let base = tempdir().unwrap();
    setup(base.path());
    let engine = MvccSqlEngine::open(base.path()).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.u(id, email, note) VALUES(1, 'a@x', 'one')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE t.public.u SET email='a@x' WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
}

#[test]
fn mvcc_insert_distinct_unique_value_succeeds() {
    let base = tempdir().unwrap();
    setup(base.path());
    let engine = MvccSqlEngine::open(base.path()).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.u(id, email, note) VALUES(1, 'a@x', 'one')")?;
        s.execute("INSERT INTO t.public.u(id, email, note) VALUES(2, 'b@x', 'two')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
}
