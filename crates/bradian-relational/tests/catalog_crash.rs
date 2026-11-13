//! Isolated process-exit recovery: no unrelated fixture FDs exist when spawning.
use bradian_relational::{Column, DataType, PrimaryKey, TableSpec};
use bradian_relational::persistent_catalog::PersistentCatalog;
use tempfile::tempdir;

fn revision_fixture(catalog: &PersistentCatalog) -> (bradian_relational::TableId, bradian_relational::Table) {
    let database = catalog.create_database("revisions").unwrap();
    let schema = catalog.create_schema(database, "public").unwrap();
    let id = catalog.create_table(schema, TableSpec {
        name: "items".into(),
        columns: vec![Column { name: "id".into(), data_type: DataType::Int64, nullable: false }],
        primary_key: PrimaryKey { columns: vec!["id".into()] }, unique_keys: vec![],
    }).unwrap();
    let mut proposed = catalog.snapshot().table(id).unwrap().clone();
    proposed.columns.push(Column { name: "notes".into(), data_type: DataType::Text, nullable: true });
    (id, proposed)
}

#[test]
fn revision_decision_recovers_after_process_exit_without_destructors() {
    const CHILD_PATH: &str = "BRADIAN_REVISION_CRASH_TEST_PATH";
    if let Some(root) = std::env::var_os(CHILD_PATH) {
        let catalog = PersistentCatalog::open(root).unwrap();
        let (id, proposed) = revision_fixture(&catalog);
        catalog.propose_table_revision(id, proposed).unwrap();
        catalog.checkpoint().unwrap();
        catalog.activate_table_revision(catalog.pending_table_revision(id).unwrap()).unwrap();
        // Bypass Rust destructors and any orderly close/checkpoint path.
        std::process::exit(77);
    }
    let dir = tempdir().unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "revision_decision_recovers_after_process_exit_without_destructors"])
        .env(CHILD_PATH, dir.path())
        .status().unwrap();
    assert_eq!(status.code(), Some(77));
    let catalog = PersistentCatalog::open(dir.path()).unwrap();
    let live = catalog.snapshot();
    let table = live.tables().next().unwrap();
    assert_eq!(table.columns.last().unwrap().name, "notes");
    assert!(catalog.pending_table_revision(table.id).is_none());
    assert_eq!(catalog.last_lsn(), 6);
}
