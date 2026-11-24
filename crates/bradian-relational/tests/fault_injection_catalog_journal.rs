//! ADR 0253 — fault injection on the PersistentCatalog
//! journal. Same middle-flip / tail-truncate / zeroed-magic
//! pattern as the RowStore WAL tests.

use bradian_relational::fault_injection::{
    file_len, flip_byte_at, truncate_file_to, zero_range,
};
use bradian_relational::persistent_catalog::{PersistentCatalog, PersistentCatalogError};
use bradian_relational::{Column, DataType, PrimaryKey, TableSpec};
use tempfile::tempdir;

/// Seed a catalog with a database, schema, and one table.
/// Returns the path to the journal WAL after close.
fn seeded_catalog_wal(dir: &std::path::Path) -> std::path::PathBuf {
    let cat = PersistentCatalog::open(dir).unwrap();
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
    drop(cat);
    dir.join("rows.wal")
}

#[test]
fn clean_reopen_matches_committed_schema() {
    let dir = tempdir().unwrap();
    let _wal = seeded_catalog_wal(dir.path());
    let cat = PersistentCatalog::open(dir.path()).unwrap();
    let snap = cat.snapshot();
    assert_eq!(snap.databases().count(), 1);
    assert_eq!(snap.schemas().count(), 1);
    assert_eq!(snap.tables().count(), 1);
}

#[test]
fn flipping_a_middle_catalog_wal_byte_is_detected() {
    let dir = tempdir().unwrap();
    let wal = seeded_catalog_wal(dir.path());
    let len = file_len(&wal).unwrap();
    flip_byte_at(&wal, len / 2).unwrap();
    let err = PersistentCatalog::open(dir.path())
        .expect_err("flipped byte must not open cleanly");
    assert!(
        matches!(
            err,
            PersistentCatalogError::Storage(_) | PersistentCatalogError::Corruption(_)
                | PersistentCatalogError::Encoding(_) | PersistentCatalogError::Catalog(_),
        ),
        "expected Storage/Corruption/Encoding/Catalog, got {err:?}",
    );
}

#[test]
fn truncating_catalog_tail_either_drops_or_rejects() {
    let dir = tempdir().unwrap();
    let wal = seeded_catalog_wal(dir.path());
    let len = file_len(&wal).unwrap();
    truncate_file_to(&wal, len - 1).unwrap();
    match PersistentCatalog::open(dir.path()) {
        Ok(cat) => {
            // Partial recovery may drop the trailing event. The
            // earlier database must still exist — it's much
            // earlier than the tail byte.
            let snap = cat.snapshot();
            assert_eq!(snap.databases().count(), 1,
                "earlier committed database lost after tail truncation");
        }
        Err(err) => assert!(
            matches!(
                err,
                PersistentCatalogError::Storage(_) | PersistentCatalogError::Corruption(_)
                    | PersistentCatalogError::Encoding(_) | PersistentCatalogError::Catalog(_),
            ),
            "expected Storage/Corruption/Encoding/Catalog, got {err:?}",
        ),
    }
}

#[test]
fn zeroing_catalog_wal_magic_is_rejected() {
    let dir = tempdir().unwrap();
    let wal = seeded_catalog_wal(dir.path());
    zero_range(&wal, 0, 4).unwrap();
    let err = PersistentCatalog::open(dir.path())
        .expect_err("zeroed magic must not open cleanly");
    assert!(
        matches!(
            err,
            PersistentCatalogError::Storage(_) | PersistentCatalogError::Corruption(_),
        ),
        "expected Storage/Corruption, got {err:?}",
    );
}

#[test]
fn deleting_catalog_wal_resets_to_empty() {
    let dir = tempdir().unwrap();
    let wal = seeded_catalog_wal(dir.path());
    std::fs::remove_file(&wal).unwrap();
    let cat = PersistentCatalog::open(dir.path()).unwrap();
    let snap = cat.snapshot();
    assert_eq!(snap.databases().count(), 0);
}
