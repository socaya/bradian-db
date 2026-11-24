//! ADR 0294 — commit receipt → `IncrementalAggregate` parity.
//!
//! Feeds a stream of `CommitReceipt::changes_by_table`
//! entries from a shared-domain two-commit mixed workload
//! into an `IncrementalAggregate::Sum` keyed by a non-PK
//! grouping column, and asserts the running incremental
//! snapshot matches a fresh scan-based reaggregation after
//! each commit.
//!
//! Covers Insert, Update (replace), and Delete events.

use std::collections::BTreeMap;

use bradian_core::Value as CoreValue;
use bradian_relational::transaction_domain::{
    CommitReceipt, CommitRowChange, MultiTableStore,
};
use bradian_relational::typed_table::Value;
use bradian_relational::{Catalog, Column, DataType, PrimaryKey, Table, TableSpec};
use bradian_views::incremental::{AggKind, IncrementalAggregate};

fn employees_table() -> Table {
    let mut catalog = Catalog::new();
    let db = catalog.create_database("hr").unwrap();
    let schema = catalog.create_schema(db, "public").unwrap();
    let id = catalog.create_table(schema, TableSpec {
        name: "employees".into(),
        columns: vec![
            Column { name: "id".into(),         data_type: DataType::Int64, nullable: false },
            Column { name: "department".into(), data_type: DataType::Text,  nullable: false },
            Column { name: "salary".into(),     data_type: DataType::Int64, nullable: false },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
        ..Default::default()
    }).unwrap();
    catalog.table(id).unwrap().clone()
}

fn row(id: i64, department: &str, salary: i64) -> Vec<Value> {
    vec![Value::Int64(id), Value::String(department.into()), Value::Int64(salary)]
}

/// Project a relational row to `(group_key, metric)` for a
/// SUM(salary) GROUP BY department view.
fn project_row(r: &[Value]) -> (Vec<CoreValue>, f64) {
    let dept = match &r[1] { Value::String(s) => s.clone(), _ => unreachable!() };
    let salary = match &r[2] { Value::Int64(v) => *v as f64, _ => unreachable!() };
    (vec![CoreValue::Str(dept)], salary)
}

/// Per-group live-row counter maintained alongside the
/// aggregate so the test can call `remove_key` once a
/// group is drained. `IncrementalAggregate` deliberately
/// retains a cell at 0.0 after all contributions are
/// retracted — governed cleanup is the caller's job (see
/// ADR 0294 Decision; `incremental.rs` docs). The counter
/// approximates what a materialised view would track.
#[derive(Default)]
struct LiveCounts(BTreeMap<Vec<CoreValue>, i64>);

impl LiveCounts {
    fn apply(&mut self, key: &[CoreValue], delta: i64) -> i64 {
        let entry = self.0.entry(key.to_vec()).or_insert(0);
        *entry += delta;
        *entry
    }
}

/// Feed one commit receipt's changes into an invertible
/// aggregate as retract-apply pairs. `live` tracks live
/// row counts per group; when a group drains, the test
/// calls `remove_key` so the aggregate snapshot matches
/// a from-scratch rescan.
fn apply_receipt(
    agg: &mut IncrementalAggregate,
    live: &mut LiveCounts,
    receipt: &CommitReceipt,
    table_id: bradian_relational::TableId,
) {
    let Some(changes) = receipt.changes_by_table.get(&table_id) else { return; };
    for change in changes {
        match change {
            CommitRowChange::Insert { new } => {
                let (key, value) = project_row(new);
                agg.apply_change(key.clone(), None, Some(value)).unwrap();
                live.apply(&key, 1);
            }
            CommitRowChange::Update { old, new } => {
                let (old_key, old_val) = project_row(old);
                let (new_key, new_val) = project_row(new);
                if old_key == new_key {
                    agg.apply_change(old_key, Some(old_val), Some(new_val)).unwrap();
                } else {
                    agg.apply_change(old_key.clone(), Some(old_val), None).unwrap();
                    agg.apply_change(new_key.clone(), None, Some(new_val)).unwrap();
                    if live.apply(&old_key, -1) == 0 { agg.remove_key(&old_key); }
                    live.apply(&new_key, 1);
                }
            }
            CommitRowChange::Delete { old } => {
                let (key, value) = project_row(old);
                agg.apply_change(key.clone(), Some(value), None).unwrap();
                if live.apply(&key, -1) == 0 { agg.remove_key(&key); }
            }
        }
    }
}

/// Fresh scan-based SUM(salary) GROUP BY department over
/// the current committed state.
fn rescan_sum_by_department(rows: &[Vec<Value>]) -> BTreeMap<String, f64> {
    let mut out: BTreeMap<String, f64> = BTreeMap::new();
    for r in rows {
        let (dept, salary) = match (&r[1], &r[2]) {
            (Value::String(s), Value::Int64(v)) => (s.clone(), *v as f64),
            _ => unreachable!(),
        };
        *out.entry(dept).or_insert(0.0) += salary;
    }
    out
}

fn snapshot_as_btree(agg: &IncrementalAggregate) -> BTreeMap<String, f64> {
    let mut out: BTreeMap<String, f64> = BTreeMap::new();
    for (key, value) in agg.snapshot() {
        let dept = match &key[0] { CoreValue::Str(s) => s.clone(), _ => unreachable!() };
        if let Some(v) = value { out.insert(dept, v); }
    }
    out
}

#[test]
fn commit_receipts_drive_incremental_sum_in_parity_with_rescan() {
    let dir = tempfile::tempdir().unwrap();
    let employees = employees_table();
    let db = MultiTableStore::open(dir.path()).unwrap();
    db.register_table(employees.clone()).unwrap();

    let mut agg = IncrementalAggregate::new(AggKind::Sum);
    let mut live = LiveCounts::default();

    // Commit 1 — three inserts.
    {
        let mut tx = db.begin_transaction().unwrap();
        tx.insert(employees.id, &row(1, "eng", 100)).unwrap();
        tx.insert(employees.id, &row(2, "eng", 200)).unwrap();
        tx.insert(employees.id, &row(3, "ops",  50)).unwrap();
        let receipt = tx.commit_with_receipt().unwrap();
        apply_receipt(&mut agg, &mut live, &receipt, employees.id);
    }
    assert_eq!(snapshot_as_btree(&agg),
               rescan_sum_by_department(&db.begin_transaction().unwrap().scan(employees.id).unwrap()),
               "parity after insert-only commit");

    // Commit 2 — replace in same group + replace across groups + insert.
    {
        let mut tx = db.begin_transaction().unwrap();
        tx.replace(employees.id, &row(1, "eng", 150)).unwrap(); // same group
        tx.replace(employees.id, &row(2, "ops", 200)).unwrap(); // group change
        tx.insert (employees.id, &row(4, "ops",  75)).unwrap();
        let receipt = tx.commit_with_receipt().unwrap();
        apply_receipt(&mut agg, &mut live, &receipt, employees.id);
    }
    assert_eq!(snapshot_as_btree(&agg),
               rescan_sum_by_department(&db.begin_transaction().unwrap().scan(employees.id).unwrap()),
               "parity after mixed-replace commit");

    // Commit 3 — delete + insert.
    {
        let mut tx = db.begin_transaction().unwrap();
        tx.delete(employees.id, &[Value::Int64(3)]).unwrap();
        tx.insert(employees.id, &row(5, "eng", 300)).unwrap();
        let receipt = tx.commit_with_receipt().unwrap();
        apply_receipt(&mut agg, &mut live, &receipt, employees.id);
    }
    assert_eq!(snapshot_as_btree(&agg),
               rescan_sum_by_department(&db.begin_transaction().unwrap().scan(employees.id).unwrap()),
               "parity after delete+insert commit");

    // Commit 4 — delete the only remaining row of a group.
    {
        let mut tx = db.begin_transaction().unwrap();
        tx.delete(employees.id, &[Value::Int64(2)]).unwrap();
        tx.delete(employees.id, &[Value::Int64(4)]).unwrap();
        let receipt = tx.commit_with_receipt().unwrap();
        apply_receipt(&mut agg, &mut live, &receipt, employees.id);
    }
    let final_committed = db.begin_transaction().unwrap().scan(employees.id).unwrap();
    assert_eq!(snapshot_as_btree(&agg),
               rescan_sum_by_department(&final_committed),
               "parity after delete-only commit");

    // Spot-check: eng remains at 150 + 300 = 450, ops drained.
    let final_sums = rescan_sum_by_department(&final_committed);
    assert_eq!(final_sums.get("eng"), Some(&450.0));
    assert_eq!(final_sums.get("ops"), None,
        "ops group should have zero live rows");
}
