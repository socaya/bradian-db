//! ADR 0266 + 0267 — snapshot integration into DomainSqlEngine
//! and MvccSqlEngine sessions.

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::mvcc_sql::MvccSqlEngine;
use bradian_relational::sql::{SqlEngine, SqlError};
use bradian_relational::SchemaSnapshot;
use tempfile::tempdir;

fn seed_sql(dir: &std::path::Path) {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, note TEXT, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
}

#[test]
fn domain_session_detects_stale_snapshot() {
    let dir = tempdir().unwrap();
    seed_sql(dir.path());
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.items").unwrap();
    // Fabricate a snapshot with an artificially-low version
    // — reconcile must surface SchemaStale immediately.
    let live = dom.capture_schema_snapshot_of(&["t.public.items"]).unwrap();
    let stale = SchemaSnapshot::from_entries(live.entries().map(|(t, _)| (t, 0)));
    let err = dom.with_session(|s| {
        s.attach_schema_snapshot(stale);
        s.execute("BEGIN")
    }).unwrap_err();
    assert!(matches!(err, SqlError::SchemaStale { .. }), "got {err:?}");
}

#[test]
fn domain_session_no_snapshot_is_unchanged() {
    let dir = tempdir().unwrap();
    seed_sql(dir.path());
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.items").unwrap();
    // No snapshot attached — ordinary session works.
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.items(id, note) VALUES(1, 'a')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
}

#[test]
fn mvcc_session_detects_stale_snapshot() {
    let dir = tempdir().unwrap();
    seed_sql(dir.path());
    {
        let dom = DomainSqlEngine::open(dir.path()).unwrap();
        dom.register_catalog_table("t.public.items").unwrap();
    }
    let mvcc = MvccSqlEngine::open(dir.path()).unwrap();
    let live = mvcc.capture_schema_snapshot_of(&["t.public.items"]).unwrap();
    let stale = SchemaSnapshot::from_entries(live.entries().map(|(t, _)| (t, 0)));
    let err = mvcc.with_session(|s| {
        s.attach_schema_snapshot(stale);
        s.execute("BEGIN")
    }).unwrap_err();
    assert!(matches!(err, SqlError::SchemaStale { .. }), "got {err:?}");
}

#[test]
fn mvcc_session_no_snapshot_is_unchanged() {
    let dir = tempdir().unwrap();
    seed_sql(dir.path());
    {
        let dom = DomainSqlEngine::open(dir.path()).unwrap();
        dom.register_catalog_table("t.public.items").unwrap();
    }
    let mvcc = MvccSqlEngine::open(dir.path()).unwrap();
    mvcc.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.items(id, note) VALUES(1, 'a')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
}
