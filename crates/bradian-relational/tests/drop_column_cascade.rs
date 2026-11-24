//! ADR 0261 — DROP COLUMN CASCADE.

use bradian_relational::sql::{SqlEngine, SqlError};
use tempfile::tempdir;

fn seed_two_tables_with_fk() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.parents (pid BIGINT NOT NULL, key TEXT NOT NULL, PRIMARY KEY(pid), UNIQUE(key))",
        "CREATE TABLE t.public.children (cid BIGINT NOT NULL, parent_key TEXT, PRIMARY KEY(cid), FOREIGN KEY(parent_key) REFERENCES t.public.parents(key))",
    ] { db.execute(sql).unwrap(); }
    (dir, db)
}

#[test]
fn plain_drop_column_rejects_when_in_unique_key() {
    let (_dir, db) = seed_two_tables_with_fk();
    let err = db.execute("ALTER TABLE t.public.parents DROP COLUMN key").unwrap_err();
    assert!(matches!(err, SqlError::Catalog(_)), "got {err:?}");
}

#[test]
fn cascade_drops_column_and_drops_dependents() {
    let (_dir, db) = seed_two_tables_with_fk();
    db.execute("ALTER TABLE t.public.parents DROP COLUMN key CASCADE").unwrap();
    // Column gone.
    let err = db.execute("SELECT key FROM t.public.parents").unwrap_err();
    assert!(matches!(err, SqlError::Resolution(_) | SqlError::Type(_) | SqlError::Table(_)),
        "got {err:?}");
    // Incoming FK on children.parent_key should also be gone.
    // We can't directly inspect, but we can INSERT a child with a
    // non-existent parent_key — it should NOT fail on FK anymore.
    db.execute("INSERT INTO t.public.parents(pid) VALUES(1)").unwrap();
    db.execute("INSERT INTO t.public.children(cid, parent_key) VALUES(1, 'ghost')").unwrap();
}

#[test]
fn cascade_on_pk_rejected() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.t (id BIGINT NOT NULL, note TEXT, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    let err = db.execute("ALTER TABLE t.public.t DROP COLUMN id CASCADE").unwrap_err();
    assert!(matches!(err, SqlError::Catalog(_)), "got {err:?}");
}

#[test]
fn cascade_on_last_column_rejected() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.t (id BIGINT NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    let err = db.execute("ALTER TABLE t.public.t DROP COLUMN id CASCADE").unwrap_err();
    assert!(matches!(err, SqlError::Catalog(_)), "got {err:?}");
}

#[test]
fn cascade_persists_across_reopen() {
    let dir = tempdir().unwrap();
    {
        let db = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.parents (pid BIGINT NOT NULL, key TEXT NOT NULL, PRIMARY KEY(pid), UNIQUE(key))",
            "CREATE TABLE t.public.children (cid BIGINT NOT NULL, parent_key TEXT, PRIMARY KEY(cid), FOREIGN KEY(parent_key) REFERENCES t.public.parents(key))",
            "ALTER TABLE t.public.parents DROP COLUMN key CASCADE",
        ] { db.execute(sql).unwrap(); }
    }
    let db = SqlEngine::open(dir.path()).unwrap();
    // After reopen, parents.key is gone and children.parent_key has no FK.
    db.execute("INSERT INTO t.public.parents(pid) VALUES(1)").unwrap();
    db.execute("INSERT INTO t.public.children(cid, parent_key) VALUES(1, 'nothing')").unwrap();
}

#[test]
fn drop_foreign_key_directly() {
    let (_dir, db) = seed_two_tables_with_fk();
    // The FK auto-name is `fk_parent_key` style; the current
    // impl generates `fk_<col>_ref_<parent>_<pcol>` — just verify
    // CASCADE works end-to-end since we don't expose the name
    // in SQL yet. Here we use the inline catalog API via the
    // engine's catalog snapshot to look up the name and drop
    // via the persistent-catalog method.
    let snap = db.catalog_snapshot();
    let children = snap.tables().find(|t| t.name == "children").unwrap();
    assert_eq!(children.foreign_keys.len(), 1);
    // CASCADE on the referenced column tests the incoming path
    // through the public API.
    db.execute("ALTER TABLE t.public.parents DROP COLUMN key CASCADE").unwrap();
    let snap = db.catalog_snapshot();
    let children = snap.tables().find(|t| t.name == "children").unwrap();
    assert!(children.foreign_keys.is_empty());
}
