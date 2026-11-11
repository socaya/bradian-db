//! Integration: DSL request → logical plan → QueryExecutor → verify against the oracle.

use std::sync::Arc;

use dap_catalog::{Catalog, MetadataObject, OrgUnit};
use dap_catalog::metadata::Dataset;
use dap_core::{Column, LogicalType, PeriodType, Schema, Uid, Value};
use dap_planner::dsl::{
    AggregateQuery, AggregationSpec, FilterSpec, OrgUnitSelector, PeriodSpec, QueryRequest,
};
use dap_planner::plan::{LogicalPlan, PlanContext, PlanOptions};
use dap_planner::runner::QueryExecutor;
use dap_runtime::budget::ExecContext;
use dap_runtime::oracle::aggregate_oracle;
use dap_storage::{Partition, PartitionOptions};
use tempfile::tempdir;

fn fact_schema() -> Schema {
    Schema::new(vec![
        Column::new("ou", LogicalType::Uid).required(),
        Column::new("period", LogicalType::Period).required(),
        Column::new("de", LogicalType::Uid).required(),
        Column::new("value", LogicalType::F64),
    ])
    .unwrap()
}

fn uid(s: &str) -> Uid {
    Uid::new(s).unwrap()
}

fn seed_catalog() -> Arc<Catalog> {
    let cat = Catalog::new();
    cat.apply(vec![
        MetadataObject::Dataset(Dataset {
            uid: uid("dsAAAAAAAAA"),
            name: "D".into(),
            period_type: PeriodType::Monthly,
            data_elements: vec![uid("deAAAAAAAAA"), uid("deBBBBBBBBB")],
        }),
        MetadataObject::OrgUnit(OrgUnit {
            uid: uid("ouROOT00000"),
            code: None,
            name: "root".into(),
            parent: None,
            level: 0,
            path: vec![uid("ouROOT00000")],
        }),
        MetadataObject::OrgUnit(OrgUnit {
            uid: uid("ouAAAAAAAAA"),
            code: None,
            name: "A".into(),
            parent: Some(uid("ouROOT00000")),
            level: 1,
            path: vec![uid("ouROOT00000"), uid("ouAAAAAAAAA")],
        }),
        MetadataObject::OrgUnit(OrgUnit {
            uid: uid("ouBBBBBBBBB"),
            code: None,
            name: "B".into(),
            parent: Some(uid("ouROOT00000")),
            level: 1,
            path: vec![uid("ouROOT00000"), uid("ouBBBBBBBBB")],
        }),
    ])
    .unwrap();
    Arc::new(cat)
}

fn seed_partition(root: &std::path::Path) -> Arc<Partition> {
    let p = Partition::open(
        root,
        PartitionOptions {
            dataset_id: "dsAAAAAAAAA".into(),
            partition_id: "p1".into(),
            schema: fact_schema(),
            primary_key_columns: vec!["ou".into(), "period".into(), "de".into()],
            flush_rows: 1024,
            flush_bytes: 1 << 24,
        },
    )
    .unwrap();

    let rows: &[(&str, &str, &str, Option<f64>)] = &[
        ("ouAAAAAAAAA", "202601", "deAAAAAAAAA", Some(10.0)),
        ("ouAAAAAAAAA", "202601", "deBBBBBBBBB", Some(5.0)),
        ("ouBBBBBBBBB", "202601", "deAAAAAAAAA", Some(20.0)),
        ("ouBBBBBBBBB", "202601", "deBBBBBBBBB", None),
        ("ouAAAAAAAAA", "202602", "deAAAAAAAAA", Some(12.0)),
        ("ouAAAAAAAAA", "202602", "deBBBBBBBBB", Some(8.0)),
        ("ouBBBBBBBBB", "202602", "deAAAAAAAAA", Some(25.0)),
    ];
    for (ou, per, de, v) in rows {
        let value = match v {
            Some(f) => Value::F64(*f),
            None => Value::Null,
        };
        p.upsert_row(
            vec![
                Value::Str((*ou).into()),
                Value::Str((*per).into()),
                Value::Str((*de).into()),
                value,
            ],
            1,
        )
        .unwrap();
    }
    p.flush().unwrap();
    p
}

#[test]
fn end_to_end_aggregate_by_ou_and_period() {
    let dir = tempdir().unwrap();
    let catalog = seed_catalog();
    let part = seed_partition(dir.path());

    let request = QueryRequest::Aggregate(AggregateQuery {
        dataset: uid("dsAAAAAAAAA"),
        partition: "p1".into(),
        measures: vec![AggregationSpec::Sum("value".into())],
        group_by: vec!["ou".into(), "period".into()],
        period: Some(PeriodSpec::Range {
            period_type: PeriodType::Monthly,
            start: "202601".into(),
            end: "202602".into(),
        }),
        org_unit: Some(OrgUnitSelector::Subtree(vec![uid("ouROOT00000")])),
        filters: vec![FilterSpec::IsNotNull { column: "value".into() }],
        order_by: vec![],
        limit: None,
        consistency: Default::default(),
    });

    let ctx = PlanContext {
        catalog,
        options: PlanOptions::default(),
    };
    let plan = LogicalPlan::build(&ctx, &request, part.clone()).unwrap();
    let exec = ExecContext::default();
    let result = QueryExecutor::execute(&plan, &exec).unwrap();

    // Convert QueryResult rows into a BTreeMap<(ou, period), sum>.
    let mut got = std::collections::BTreeMap::new();
    for r in 0..result.batch.row_count() {
        let row = result.batch.row_values(r).unwrap();
        got.insert((row[0].clone(), row[1].clone()), row[2].clone());
    }
    let expected: Vec<((&str, &str), f64)> = vec![
        (("ouAAAAAAAAA", "202601"), 15.0),
        (("ouAAAAAAAAA", "202602"), 20.0),
        (("ouBBBBBBBBB", "202601"), 20.0),
        (("ouBBBBBBBBB", "202602"), 25.0),
    ];
    for ((ou, per), sum) in expected {
        let key = (Value::Str(ou.into()), Value::Str(per.into()));
        let v = got.get(&key).unwrap_or_else(|| {
            panic!("missing key {key:?}. all: {got:#?}")
        });
        assert_eq!(v, &Value::F64(sum));
    }

    // Verify provenance is populated.
    assert!(result.provenance.data_revision > 0);
    assert!(result.provenance.is_exact);
}

#[test]
fn period_type_mismatch_is_rejected() {
    let dir = tempdir().unwrap();
    let catalog = seed_catalog();
    let part = seed_partition(dir.path());

    let request = QueryRequest::Aggregate(AggregateQuery {
        dataset: uid("dsAAAAAAAAA"),
        partition: "p1".into(),
        measures: vec![AggregationSpec::CountStar],
        group_by: vec!["ou".into()],
        period: Some(PeriodSpec::Range {
            period_type: PeriodType::Quarterly,
            start: "2026Q1".into(),
            end: "2026Q2".into(),
        }),
        org_unit: None,
        filters: vec![],
        order_by: vec![],
        limit: None,
        consistency: Default::default(),
    });

    let ctx = PlanContext {
        catalog,
        options: PlanOptions::default(),
    };
    let err = LogicalPlan::build(&ctx, &request, part).expect_err("mismatch rejected");
    let msg = err.to_string();
    assert!(msg.contains("Quarterly") || msg.contains("Monthly"), "got: {msg}");
}

#[test]
fn oracle_parity_holds_over_random_filter() {
    let dir = tempdir().unwrap();
    let catalog = seed_catalog();
    let part = seed_partition(dir.path());

    // Build the same aggregate via oracle + executor.
    let request = QueryRequest::Aggregate(AggregateQuery {
        dataset: uid("dsAAAAAAAAA"),
        partition: "p1".into(),
        measures: vec![AggregationSpec::Sum("value".into())],
        group_by: vec!["period".into()],
        period: None,
        org_unit: None,
        filters: vec![FilterSpec::In {
            column: "de".into(),
            values: vec![Value::Str("deAAAAAAAAA".into())],
        }],
        order_by: vec![],
        limit: None,
        consistency: Default::default(),
    });

    let ctx = PlanContext {
        catalog,
        options: PlanOptions::default(),
    };
    let plan = LogicalPlan::build(&ctx, &request, part.clone()).unwrap();
    let exec = ExecContext::default();
    let executor_out = QueryExecutor::execute(&plan, &exec).unwrap();

    // Oracle re-execute over the materialised scan.
    let scan_batches = dap_runtime::scan::scan_partition(
        &part,
        &dap_runtime::scan::ScanOptions::default(),
        &exec,
    )
    .unwrap();
    let oracle = aggregate_oracle(
        &scan_batches,
        &plan.predicate,
        &plan.group_by,
        &plan.aggregates,
    )
    .unwrap();

    let mut exec_map = std::collections::BTreeMap::new();
    for r in 0..executor_out.batch.row_count() {
        let row = executor_out.batch.row_values(r).unwrap();
        exec_map.insert(row[0].clone(), row[1].clone());
    }
    let mut oracle_map = std::collections::BTreeMap::new();
    for (k, vs) in oracle {
        oracle_map.insert(k[0].clone(), vs[0].clone());
    }
    assert_eq!(exec_map, oracle_map);
}
