//! Integration: one coordinator, two in-process workers, one logical fragment.

use std::sync::Arc;

use dap_catalog::metadata::Dataset;
use dap_catalog::{Catalog, MetadataObject, OrgUnit};
use dap_core::{Column, LogicalType, PeriodType, Schema, Uid, Value};
use dap_distributed::{
    Coordinator, Fragment, FragmentKind, InProcessWorker, WorkerCapabilities, WorkerId,
    WorkerRegistry,
};
use dap_planner::dsl::{
    AggregateQuery, AggregationSpec, OrgUnitSelector, PeriodSpec, QueryConsistency,
};
use dap_planner::plan::{LogicalPlan, PlanContext, PlanOptions};
use dap_runtime::budget::ExecContext;
use dap_storage::{Partition, PartitionOptions};
use tempfile::tempdir;

fn uid(s: &str) -> Uid {
    Uid::new(s).unwrap()
}

fn seed_site(root: &std::path::Path, ou: &str, value: f64) -> Arc<Partition> {
    let schema = Schema::new(vec![
        Column::new("ou", LogicalType::Uid).required(),
        Column::new("period", LogicalType::Period).required(),
        Column::new("de", LogicalType::Uid).required(),
        Column::new("value", LogicalType::F64),
    ])
    .unwrap();
    let p = Partition::open(
        root,
        PartitionOptions {
            dataset_id: "dsAAAAAAAAA".into(),
            partition_id: "p1".into(),
            schema,
            primary_key_columns: vec!["ou".into(), "period".into(), "de".into()],
            flush_rows: 1024,
            flush_bytes: 1 << 24,
        },
    )
    .unwrap();
    p.upsert_row(
        vec![
            Value::Str(ou.into()),
            Value::Str("202601".into()),
            Value::Str("deAAAAAAAAA".into()),
            Value::F64(value),
        ],
        1,
    )
    .unwrap();
    p.flush().unwrap();
    p
}

fn seed_catalog() -> Arc<Catalog> {
    let cat = Catalog::new();
    cat.apply(vec![
        MetadataObject::Dataset(Dataset {
            uid: uid("dsAAAAAAAAA"),
            name: "D".into(),
            period_type: PeriodType::Monthly,
            data_elements: vec![uid("deAAAAAAAAA")],
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

#[test]
fn coordinator_dispatches_to_two_workers_and_merges() {
    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    let part_a = seed_site(dir_a.path(), "ouAAAAAAAAA", 42.0);
    let part_b = seed_site(dir_b.path(), "ouBBBBBBBBB", 100.0);
    let catalog = seed_catalog();

    let caps_a = WorkerCapabilities {
        datasets: vec!["dsAAAAAAAAA".into()],
        programs: vec![],
        max_fragment_rows: 10_000,
        supports_cross_site_join: false,
        location: "siteA".into(),
    };
    let caps_b = caps_a.clone();

    let catalog_a = catalog.clone();
    let worker_a = Arc::new(InProcessWorker::new(
        WorkerId::new("siteA"),
        caps_a,
        Arc::new(move |frag: &Fragment| match &frag.kind {
            FragmentKind::Aggregate(aq) => LogicalPlan::build(
                &PlanContext {
                    catalog: catalog_a.clone(),
                    options: PlanOptions::default(),
                },
                &dap_planner::dsl::QueryRequest::Aggregate(aq.clone()),
                part_a.clone(),
            ),
            _ => Err(dap_core::DapError::other("site A only serves aggregate")),
        }),
    ));
    let catalog_b = catalog.clone();
    let worker_b = Arc::new(InProcessWorker::new(
        WorkerId::new("siteB"),
        caps_b,
        Arc::new(move |frag: &Fragment| match &frag.kind {
            FragmentKind::Aggregate(aq) => LogicalPlan::build(
                &PlanContext {
                    catalog: catalog_b.clone(),
                    options: PlanOptions::default(),
                },
                &dap_planner::dsl::QueryRequest::Aggregate(aq.clone()),
                part_b.clone(),
            ),
            _ => Err(dap_core::DapError::other("site B only serves aggregate")),
        }),
    ));

    let registry = Arc::new(WorkerRegistry::new());
    registry.register(worker_a);
    registry.register(worker_b);
    let coord = Coordinator::new(registry);

    let aq = AggregateQuery {
        dataset: uid("dsAAAAAAAAA"),
        partition: "p1".into(),
        measures: vec![AggregationSpec::Sum("value".into())],
        group_by: vec!["ou".into()],
        period: Some(PeriodSpec::Exact("202601".into())),
        org_unit: Some(OrgUnitSelector::Subtree(vec![uid("ouROOT00000")])),
        filters: vec![],
        order_by: vec![],
        limit: None,
        consistency: QueryConsistency::default(),
    };
    let fragment = Fragment {
        id: "frag1".into(),
        kind: FragmentKind::Aggregate(aq),
        ou_scope: vec![uid("ouROOT00000")],
    };

    let ctx = ExecContext::default();
    let result = coord.execute(fragment, &ctx).unwrap();
    // Two complete sites: A contributes ou=A sum=42; B contributes ou=B sum=100. Merged
    // batch has two rows.
    assert_eq!(result.batch.row_count(), 2);
    assert_eq!(result.complete_sites.len(), 2);
    assert!(result.partial_sites.is_empty());
    assert!(result.provenance.is_exact);
}
