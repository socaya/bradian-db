//! ADR 0097 — reader-session schema pin registry.

use bradian_relational::persistent_catalog::PersistentCatalog;
use bradian_relational::{Column, DataType, PrimaryKey, TableSpec};
use tempfile::tempdir;

fn seed(cat: &PersistentCatalog) -> bradian_relational::DatabaseId {
    cat.create_database("db").unwrap()
}

#[test]
fn ddl_does_not_invalidate_active_reader_snapshot() {
    let dir = tempdir().unwrap();
    let cat = PersistentCatalog::open(dir.path()).unwrap();
    let db = seed(&cat);
    let sess = cat.reader_session();
    let r0 = sess.revision();
    // Snapshot sees exactly one database.
    assert_eq!(sess.snapshot().databases().count(), 1);

    // DDL after the session begins.
    let _db2 = cat.create_database("db2").unwrap();
    let schema = cat.create_schema(db, "public").unwrap();
    let _t = cat.create_table(schema, TableSpec {
        name: "t".into(),
        columns: vec![Column { name: "id".into(), data_type: DataType::Int64, nullable: false }],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
    ..Default::default()
    }).unwrap();

    // Session's snapshot is frozen — still sees exactly one DB.
    assert_eq!(sess.snapshot().databases().count(), 1);
    assert_eq!(sess.revision(), r0);
    // Live catalog has advanced.
    assert!(cat.last_lsn() > r0);
}

#[test]
fn active_reader_revisions_reflects_pin_lifecycle() {
    let dir = tempdir().unwrap();
    let cat = PersistentCatalog::open(dir.path()).unwrap();
    let _db = seed(&cat);

    assert!(cat.active_reader_revisions().is_empty());
    let s1 = cat.reader_session();
    let r1 = s1.revision();
    assert_eq!(cat.active_reader_revisions().get(&r1), Some(&1));
    let s2 = cat.reader_session();
    assert_eq!(s2.revision(), r1, "two sessions at same revision coalesce");
    assert_eq!(cat.active_reader_revisions().get(&r1), Some(&2));

    drop(s1);
    assert_eq!(cat.active_reader_revisions().get(&r1), Some(&1));
    drop(s2);
    assert!(cat.active_reader_revisions().is_empty(),
        "last session drop removes the revision key");
}

#[test]
fn sessions_at_different_revisions_coexist_independently() {
    let dir = tempdir().unwrap();
    let cat = PersistentCatalog::open(dir.path()).unwrap();
    let _db = seed(&cat);
    let old = cat.reader_session();
    let r_old = old.revision();
    // Advance the catalog.
    let _db2 = cat.create_database("db2").unwrap();
    let new = cat.reader_session();
    let r_new = new.revision();
    assert!(r_new > r_old);

    let regs = cat.active_reader_revisions();
    assert_eq!(regs.get(&r_old), Some(&1));
    assert_eq!(regs.get(&r_new), Some(&1));
    assert_eq!(regs.len(), 2);

    drop(old);
    let regs = cat.active_reader_revisions();
    assert!(!regs.contains_key(&r_old));
    assert_eq!(regs.get(&r_new), Some(&1));
}
