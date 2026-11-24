//! ADR 0255 — extend the ADR 0254 property sweep to the
//! user catalog and DDL catalog journals. Same strong
//! property: for any fault, reopen either errors cleanly or
//! exposes a subset of the committed state with no
//! fabricated or corrupted data.

use bradian_relational::fault_injection::{file_len, flip_byte_at, truncate_file_to};
use bradian_relational::persistent_catalog::{PersistentCatalog, PersistentCatalogError};
use bradian_relational::persistent_user_catalog::{
    PersistentUserCatalog, PersistentUserCatalogError,
};
use bradian_relational::{Column, DataType, PrimaryKey, TableSpec};

/// Copy a directory's immediate files into a fresh tempdir.
fn clone_dir(src: &std::path::Path) -> tempfile::TempDir {
    let dst = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_file() {
            std::fs::copy(&path, dst.path().join(entry.file_name())).unwrap();
        }
    }
    dst
}

fn sample_offsets(len: u64, cap: usize) -> Vec<u64> {
    if len == 0 { return Vec::new(); }
    let mut offsets: Vec<u64> = Vec::new();
    for i in 0..4.min(len) { offsets.push(i); }
    let tail_start = len.saturating_sub(8);
    for i in tail_start..len { offsets.push(i); }
    let step = (len / cap as u64).max(1);
    let mut cursor = 0;
    while cursor < len && offsets.len() < cap {
        offsets.push(cursor);
        cursor += step;
    }
    offsets.sort();
    offsets.dedup();
    offsets
}

// -----------------------------------------------------------------
// User catalog sweep
// -----------------------------------------------------------------

fn seed_users() -> (tempfile::TempDir, std::path::PathBuf, Vec<(&'static str, &'static str)>) {
    let dir = tempfile::tempdir().unwrap();
    let cat = PersistentUserCatalog::open(dir.path()).unwrap();
    let users = vec![("alice", "pw1"), ("bob", "pw2"), ("carol", "pw3")];
    for (u, p) in &users {
        cat.create_user(u, p).unwrap();
    }
    drop(cat);
    let wal = dir.path().join("rows.wal");
    (dir, wal, users)
}

fn assert_user_subset(cat: &PersistentUserCatalog, all: &[(&str, &str)]) {
    for (u, p) in all {
        if cat.contains(u) {
            cat.verify_password(u, p)
                .unwrap_or_else(|e| panic!("user {u} present but verify failed: {e:?}"));
        }
    }
}

#[test]
fn user_journal_byte_flip_sweep() {
    let (src, wal, users) = seed_users();
    let len = file_len(&wal).unwrap();
    assert!(len > 32);
    let offsets = sample_offsets(len, 32);
    for off in &offsets {
        let scenario = clone_dir(src.path());
        flip_byte_at(&scenario.path().join("rows.wal"), *off).unwrap();
        match PersistentUserCatalog::open(scenario.path()) {
            Ok(cat) => assert_user_subset(&cat, &users),
            Err(err) => assert!(
                matches!(
                    err,
                    PersistentUserCatalogError::Storage(_)
                        | PersistentUserCatalogError::Corruption(_)
                        | PersistentUserCatalogError::Encoding(_),
                ),
                "offset {off}: expected Storage/Corruption/Encoding, got {err:?}",
            ),
        }
    }
}

#[test]
fn user_journal_tail_truncation_sweep() {
    let (src, wal, users) = seed_users();
    let len = file_len(&wal).unwrap();
    assert!(len > 32);
    let mut points: Vec<u64> = (1..20u64).map(|i| (len * i / 20).max(1)).collect();
    let tail_start = len.saturating_sub(8).max(1);
    points.extend(tail_start..len);
    points.sort();
    points.dedup();
    for p in &points {
        let scenario = clone_dir(src.path());
        truncate_file_to(&scenario.path().join("rows.wal"), *p).unwrap();
        match PersistentUserCatalog::open(scenario.path()) {
            Ok(cat) => assert_user_subset(&cat, &users),
            Err(err) => assert!(
                matches!(
                    err,
                    PersistentUserCatalogError::Storage(_)
                        | PersistentUserCatalogError::Corruption(_)
                        | PersistentUserCatalogError::Encoding(_),
                ),
                "truncate {p}: expected Storage/Corruption/Encoding, got {err:?}",
            ),
        }
    }
}

// -----------------------------------------------------------------
// DDL catalog sweep
// -----------------------------------------------------------------

fn seed_catalog() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let cat = PersistentCatalog::open(dir.path()).unwrap();
    let db = cat.create_database("app").unwrap();
    let sch = cat.create_schema(db, "public").unwrap();
    cat.create_table(sch, TableSpec {
        name: "items".into(),
        columns: vec![
            Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "note".into(), data_type: DataType::Text, nullable: true },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
        column_collations: Default::default(),
        column_checks: Default::default(),
    }).unwrap();
    cat.create_table(sch, TableSpec {
        name: "other".into(),
        columns: vec![
            Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
        column_collations: Default::default(),
        column_checks: Default::default(),
    }).unwrap();
    drop(cat);
    let wal = dir.path().join("rows.wal");
    (dir, wal)
}

/// Any table that exists after reopen must have been in the
/// seeded set. The seeded set is a superset of what the
/// corrupted reopen might expose; we assert no fabrication.
fn assert_catalog_subset(cat: &PersistentCatalog) {
    let snap = cat.snapshot();
    let allowed_dbs = ["app"];
    for db in snap.databases() {
        assert!(allowed_dbs.contains(&db.name.as_str()),
            "fabricated database {:?}", db.name);
    }
    let allowed_tables = ["items", "other"];
    for t in snap.tables() {
        assert!(allowed_tables.contains(&t.name.as_str()),
            "fabricated table {:?}", t.name);
    }
}

#[test]
fn catalog_journal_byte_flip_sweep() {
    let (src, wal) = seed_catalog();
    let len = file_len(&wal).unwrap();
    assert!(len > 32);
    let offsets = sample_offsets(len, 32);
    for off in &offsets {
        let scenario = clone_dir(src.path());
        flip_byte_at(&scenario.path().join("rows.wal"), *off).unwrap();
        match PersistentCatalog::open(scenario.path()) {
            Ok(cat) => assert_catalog_subset(&cat),
            Err(err) => assert!(
                matches!(
                    err,
                    PersistentCatalogError::Storage(_)
                        | PersistentCatalogError::Corruption(_)
                        | PersistentCatalogError::Encoding(_)
                        | PersistentCatalogError::Catalog(_),
                ),
                "offset {off}: expected Storage/Corruption/Encoding/Catalog, got {err:?}",
            ),
        }
    }
}

#[test]
fn catalog_journal_tail_truncation_sweep() {
    let (src, wal) = seed_catalog();
    let len = file_len(&wal).unwrap();
    assert!(len > 32);
    let mut points: Vec<u64> = (1..20u64).map(|i| (len * i / 20).max(1)).collect();
    let tail_start = len.saturating_sub(8).max(1);
    points.extend(tail_start..len);
    points.sort();
    points.dedup();
    for p in &points {
        let scenario = clone_dir(src.path());
        truncate_file_to(&scenario.path().join("rows.wal"), *p).unwrap();
        match PersistentCatalog::open(scenario.path()) {
            Ok(cat) => assert_catalog_subset(&cat),
            Err(err) => assert!(
                matches!(
                    err,
                    PersistentCatalogError::Storage(_)
                        | PersistentCatalogError::Corruption(_)
                        | PersistentCatalogError::Encoding(_)
                        | PersistentCatalogError::Catalog(_),
                ),
                "truncate {p}: expected Storage/Corruption/Encoding/Catalog, got {err:?}",
            ),
        }
    }
}
