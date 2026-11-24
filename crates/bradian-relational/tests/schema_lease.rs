//! ADR 0265 — schema lease registry.

use bradian_relational::sql::SqlEngine;
use tempfile::tempdir;

fn seed() -> (tempfile::TempDir, SqlEngine, bradian_relational::TableId) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, note TEXT, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    let cat = db.catalog_snapshot();
    let id = cat.tables().find(|t| t.name == "items").unwrap().id;
    (dir, db, id)
}

#[test]
fn no_leases_means_zero_active_readers() {
    let (_dir, db, id) = seed();
    assert_eq!(db.active_readers_on(id), 0);
}

#[test]
fn single_lease_pins_one_reader() {
    let (_dir, db, id) = seed();
    let _lease = db.capture_schema_lease(&["t.public.items"]).unwrap();
    assert_eq!(db.active_readers_on(id), 1);
}

#[test]
fn dropping_lease_releases_pin() {
    let (_dir, db, id) = seed();
    {
        let _lease = db.capture_schema_lease(&["t.public.items"]).unwrap();
        assert_eq!(db.active_readers_on(id), 1);
    }
    assert_eq!(db.active_readers_on(id), 0);
}

#[test]
fn multiple_leases_at_same_version_accumulate() {
    let (_dir, db, id) = seed();
    let _a = db.capture_schema_lease(&["t.public.items"]).unwrap();
    let _b = db.capture_schema_lease(&["t.public.items"]).unwrap();
    let _c = db.capture_schema_lease(&["t.public.items"]).unwrap();
    assert_eq!(db.active_readers_on(id), 3);
}

#[test]
fn leases_at_different_versions_pin_separately() {
    let (_dir, db, id) = seed();
    let _a = db.capture_schema_lease(&["t.public.items"]).unwrap();
    // ALTER advances the version — but note: the lease still
    // pins the OLD version.
    db.execute("ALTER TABLE t.public.items ALTER COLUMN note SET NOT NULL BACKFILL 'd'")
        .unwrap();
    let _b = db.capture_schema_lease(&["t.public.items"]).unwrap();
    assert_eq!(db.active_readers_on(id), 2);
    let versions = db.reader_pin_versions(id);
    // Two different versions pinned, each with count 1.
    assert_eq!(versions.len(), 2);
    for &count in versions.values() { assert_eq!(count, 1); }
}

#[test]
fn lease_carries_snapshot_for_use_with_execute() {
    let (_dir, db, _id) = seed();
    db.execute("INSERT INTO t.public.items(id, note) VALUES(1, 'a')").unwrap();
    let lease = db.capture_schema_lease(&["t.public.items"]).unwrap();
    db.execute_with_schema_snapshot(
        "SELECT id FROM t.public.items WHERE id = 1", lease.snapshot()).unwrap();
}

#[test]
fn drop_removes_table_entry_when_last_lease_goes() {
    let (_dir, db, id) = seed();
    {
        let _lease = db.capture_schema_lease(&["t.public.items"]).unwrap();
        assert_eq!(db.reader_pin_versions(id).len(), 1);
    }
    assert_eq!(db.reader_pin_versions(id).len(), 0);
}
