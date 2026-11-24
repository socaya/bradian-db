//! ADR 0295 — end-to-end LSN-based HTAP freshness.
//!
//! Opens a shared domain, maintains a view of the last
//! commit the consumer has absorbed, and asserts that
//! `HtapFreshness { view_lsn, source_lsn }` reports the
//! expected Fresh / Behind{lag} / Ahead{excess}
//! classification across two commits.

use bradian_relational::transaction_domain::MultiTableStore;
use bradian_relational::typed_table::Value;
use bradian_relational::{Catalog, Column, DataType, PrimaryKey, Table, TableSpec};
use bradian_views::{HtapFreshness, HtapStaleness};

fn items_table() -> Table {
    let mut catalog = Catalog::new();
    let db = catalog.create_database("inv").unwrap();
    let schema = catalog.create_schema(db, "public").unwrap();
    let id = catalog.create_table(schema, TableSpec {
        name: "items".into(),
        columns: vec![
            Column { name: "id".into(),  data_type: DataType::Int64, nullable: false },
            Column { name: "qty".into(), data_type: DataType::Int64, nullable: false },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
        ..Default::default()
    }).unwrap();
    catalog.table(id).unwrap().clone()
}

#[test]
fn freshness_tracks_view_vs_source_lsn_across_commits() {
    let dir = tempfile::tempdir().unwrap();
    let items = items_table();
    let db = MultiTableStore::open(dir.path()).unwrap();
    db.register_table(items.clone()).unwrap();

    // Before any commit: view and source are both at the
    // initial LSN. Fresh.
    let mut view_lsn = db.last_lsn();
    let stamp = HtapFreshness::new(view_lsn, db.last_lsn());
    assert_eq!(stamp.classify(), HtapStaleness::Fresh);
    assert!(stamp.is_fresh());
    assert_eq!(stamp.lag_lsn(), 0);

    // Commit 1: the view has NOT absorbed the receipt yet.
    let lsn1 = {
        let mut tx = db.begin_transaction().unwrap();
        tx.insert(items.id, &[Value::Int64(1), Value::Int64(10)]).unwrap();
        tx.commit_with_receipt().unwrap().lsn
    };
    let stamp = HtapFreshness::new(view_lsn, db.last_lsn());
    assert_eq!(stamp.source_lsn, lsn1);
    assert!(!stamp.is_fresh());
    assert_eq!(stamp.classify(), HtapStaleness::Behind { lag: lsn1 - view_lsn });
    assert_eq!(stamp.lag_lsn(), lsn1 - view_lsn);

    // The view consumer now absorbs receipt 1.
    view_lsn = lsn1;
    let stamp = HtapFreshness::new(view_lsn, db.last_lsn());
    assert_eq!(stamp.classify(), HtapStaleness::Fresh);

    // Commit 2.
    let lsn2 = {
        let mut tx = db.begin_transaction().unwrap();
        tx.insert(items.id, &[Value::Int64(2), Value::Int64(20)]).unwrap();
        tx.commit_with_receipt().unwrap().lsn
    };
    let stamp = HtapFreshness::new(view_lsn, db.last_lsn());
    assert_eq!(stamp.classify(), HtapStaleness::Behind { lag: lsn2 - lsn1 });
    assert_eq!(stamp.lag_lsn(), 1);
    view_lsn = lsn2;
    assert!(HtapFreshness::new(view_lsn, db.last_lsn()).is_fresh());

    // Synthesized Ahead case: a cached stamp that was taken
    // after a later commit than the one the current reader
    // observes (e.g. an expired snapshot). Reported as Ahead
    // rather than silently clamped.
    let stamp = HtapFreshness::new(view_lsn + 7, db.last_lsn());
    assert_eq!(stamp.classify(), HtapStaleness::Ahead { excess: 7 });
    assert_eq!(stamp.lag_lsn(), 0);
    assert!(!stamp.is_fresh());
}
