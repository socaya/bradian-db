//! ADR 0256 — SqlEngine::execute_with_schema_snapshot.

use bradian_relational::sql::{SqlEngine, SqlError};
use tempfile::tempdir;

fn seed() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, note TEXT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, note) VALUES(1, 'a')",
    ] { db.execute(sql).unwrap(); }
    (dir, db)
}

#[test]
fn snapshot_reconciles_without_mutation() {
    let (_dir, db) = seed();
    let snap = db.capture_schema_snapshot_of(&["t.public.items"]).unwrap();
    // No mutation — the snapshot must still match.
    db.execute_with_schema_snapshot("SELECT id FROM t.public.items WHERE id = 1", &snap)
        .unwrap();
}

#[test]
fn snapshot_detects_rename_table() {
    let (_dir, db) = seed();
    let snap = db.capture_schema_snapshot_of(&["t.public.items"]).unwrap();
    db.execute("ALTER TABLE t.public.items RENAME TO renamed").unwrap();
    // The snapshot's captured version is stale.
    let err = db.execute_with_schema_snapshot(
        "SELECT id FROM t.public.renamed WHERE id = 1", &snap).unwrap_err();
    match err {
        SqlError::SchemaStale { current, captured, .. } => {
            assert!(current > captured);
        }
        other => panic!("expected SchemaStale, got {other:?}"),
    }
}

#[test]
fn snapshot_detects_rename_column() {
    let (_dir, db) = seed();
    let snap = db.capture_schema_snapshot_of(&["t.public.items"]).unwrap();
    db.execute("ALTER TABLE t.public.items RENAME COLUMN note TO memo").unwrap();
    let err = db.execute_with_schema_snapshot(
        "SELECT id FROM t.public.items WHERE id = 1", &snap).unwrap_err();
    assert!(matches!(err, SqlError::SchemaStale { .. }), "got {err:?}");
}

#[test]
fn snapshot_detects_drop_column() {
    let (_dir, db) = seed();
    // Add a droppable column first.
    db.execute("CREATE TABLE t.public.items2 (id BIGINT NOT NULL, extra TEXT, PRIMARY KEY(id))")
        .unwrap();
    let snap = db.capture_schema_snapshot_of(&["t.public.items2"]).unwrap();
    db.execute("ALTER TABLE t.public.items2 DROP COLUMN extra").unwrap();
    let err = db.execute_with_schema_snapshot(
        "SELECT id FROM t.public.items2", &snap).unwrap_err();
    assert!(matches!(err, SqlError::SchemaStale { .. }), "got {err:?}");
}

#[test]
fn snapshot_detects_set_not_null() {
    let (_dir, db) = seed();
    let snap = db.capture_schema_snapshot_of(&["t.public.items"]).unwrap();
    db.execute("ALTER TABLE t.public.items ALTER COLUMN note SET NOT NULL BACKFILL 'default'")
        .unwrap();
    let err = db.execute_with_schema_snapshot(
        "SELECT id FROM t.public.items WHERE id = 1", &snap).unwrap_err();
    assert!(matches!(err, SqlError::SchemaStale { .. }), "got {err:?}");
}

#[test]
fn multi_table_snapshot_detects_only_altered_ones() {
    let (_dir, db) = seed();
    db.execute("CREATE TABLE t.public.other (id BIGINT NOT NULL, PRIMARY KEY(id))").unwrap();
    let snap = db.capture_schema_snapshot_of(&["t.public.items", "t.public.other"]).unwrap();
    // Alter only `other`.
    db.execute("ALTER TABLE t.public.other RENAME TO other2").unwrap();
    let err = db.execute_with_schema_snapshot(
        "SELECT id FROM t.public.items WHERE id = 1", &snap).unwrap_err();
    assert!(matches!(err, SqlError::SchemaStale { .. }), "got {err:?}");
}

#[test]
fn snapshot_capture_rejects_unknown_table() {
    let (_dir, db) = seed();
    let err = db.capture_schema_snapshot_of(&["t.public.ghost"]).unwrap_err();
    assert!(matches!(err, SqlError::Resolution(_)), "got {err:?}");
}

#[test]
fn execute_without_snapshot_still_works_after_alter() {
    // Control: the normal execute path must NOT reject on
    // schema-stale when no snapshot is supplied. Use SET NOT
    // NULL which rewrites the TableStore pin (ADR 0241), so
    // the control path doesn't hit unrelated ALTER limitations.
    let (_dir, db) = seed();
    db.execute("ALTER TABLE t.public.items ALTER COLUMN note SET NOT NULL BACKFILL 'd'")
        .unwrap();
    db.execute("SELECT id FROM t.public.items WHERE id = 1").unwrap();
}

#[test]
fn snapshot_reports_specific_table_in_error() {
    let (_dir, db) = seed();
    // Resolve items' id BEFORE the ALTER so the test can assert
    // the specific id surfaces in the error.
    let catalog_before = db.catalog_snapshot();
    let items_id = catalog_before.tables()
        .find(|t| t.name == "items").unwrap().id.0;
    let snap = db.capture_schema_snapshot_of(&["t.public.items"]).unwrap();
    db.execute("ALTER TABLE t.public.items RENAME TO renamed").unwrap();
    // The sql itself doesn't need to be parseable against the
    // live schema — reconcile fires before the parser.
    let err = db.execute_with_schema_snapshot(
        "SELECT id FROM t.public.items WHERE id = 1", &snap).unwrap_err();
    match err {
        SqlError::SchemaStale { table, .. } => assert_eq!(table, items_id),
        other => panic!("expected SchemaStale, got {other:?}"),
    }
}
