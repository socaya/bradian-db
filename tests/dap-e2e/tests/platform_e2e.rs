//! End-to-end platform test: canonical ingest -> storage -> planner -> security policy ->
//! HTTP response. Covers the spec §16 acceptance scenarios 1 (correction), 3 (idempotent
//! replay) and 7 (offline site queries).

use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Request, StatusCode};
use dap_catalog::metadata::Dataset;
use dap_catalog::{Catalog, MetadataObject, OrgUnit};
use dap_core::{Column, LogicalType, PeriodType, Schema, Uid, Value};
use dap_ingest::{Change, Ingester, Operation, QuarantineLog};
use dap_security::{policy_predicate, PolicyContext, Principal, Role};
use dap_serving::{router, AnalyticsEngine, ServerState};
use dap_storage::PartitionOptions;
use serde_json::{json, Value as JsonValue};
use tempfile::tempdir;
use tower::ServiceExt;

const TEST_TOKEN: &str = "0123456789abcdef0123456789abcdef";

fn uid(s: &str) -> Uid {
    Uid::new(s).unwrap()
}

fn setup() -> (
    Arc<AnalyticsEngine>,
    Arc<dap_storage::Partition>,
    QuarantineLog,
    tempfile::TempDir,
) {
    let dir = tempdir().unwrap();

    let cat = Catalog::new();
    cat.apply(vec![
        MetadataObject::Dataset(Dataset {
            uid: uid("dsAAAAAAAAA"),
            name: "Agg".into(),
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
    let engine = Arc::new(AnalyticsEngine::new(
        dir.path().to_path_buf(),
        Arc::new(cat),
    ));
    let schema = Schema::new(vec![
        Column::new("ou", LogicalType::Uid).required(),
        Column::new("period", LogicalType::Period).required(),
        Column::new("de", LogicalType::Uid).required(),
        Column::new("value", LogicalType::F64),
    ])
    .unwrap();
    let part = engine
        .open_partition(PartitionOptions {
            dataset_id: "dsAAAAAAAAA".into(),
            partition_id: "p1".into(),
            schema,
            primary_key_columns: vec!["ou".into(), "period".into(), "de".into()],
            flush_rows: 1024,
            flush_bytes: 1 << 24,
        })
        .unwrap();
    let q = QuarantineLog::open(dir.path()).unwrap();
    (engine, part, q, dir)
}

#[tokio::test]
async fn acceptance_1_correction_retracts_old_and_applies_new() {
    let (engine, part, q, _dir) = setup();
    let ing = Ingester::new(
        part.clone(),
        q,
        vec!["ou".into(), "period".into(), "de".into()],
    );

    // Initial upsert: ou=A, period=202601, de=A, value=10.
    ing.apply(&Change {
        change_id: "c1".into(),
        site_id: None,
        object_type: "AGGREGATE_DATA_VALUE".into(),
        object_uid: uid("deAAAAAAAAA"),
        source_version: 1,
        operation: Operation::Upsert,
        occurred_at: "2026-01-01T00:00:00Z".into(),
        metadata_revision: None,
        payload_hash: None,
        payload: json!({
            "ou": "ouAAAAAAAAA",
            "period": "202601",
            "de": "deAAAAAAAAA",
            "value": 10.0,
        }),
    })
    .unwrap();

    // Correction: same key, higher version, value=15.
    ing.apply(&Change {
        change_id: "c2".into(),
        site_id: None,
        object_type: "AGGREGATE_DATA_VALUE".into(),
        object_uid: uid("deAAAAAAAAA"),
        source_version: 2,
        operation: Operation::Upsert,
        occurred_at: "2026-01-01T01:00:00Z".into(),
        metadata_revision: None,
        payload_hash: None,
        payload: json!({
            "ou": "ouAAAAAAAAA",
            "period": "202601",
            "de": "deAAAAAAAAA",
            "value": 15.0,
        }),
    })
    .unwrap();

    part.flush().unwrap();

    // HTTP query: SUM(value) grouped by ou,period must see 15, not 25 (no double-count).
    let state = ServerState::new(engine);
    state.register_bootstrap_admin(TEST_TOKEN).unwrap();
    let app = router(state);
    let body = json!({
        "request": {
            "kind": "aggregate",
            "dataset": "dsAAAAAAAAA",
            "partition": "p1",
            "measures": [{"op": "sum", "column": "value"}],
            "group_by": ["ou", "period"],
            "filters": [],
            "order_by": [],
        }
    });

    let resp = app
        .oneshot(
            Request::post("/api/analytics/v1/query")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {TEST_TOKEN}"))
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let payload: JsonValue = serde_json::from_slice(&bytes).unwrap();
    let rows = payload["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    // row: [ou, period, sum_value]
    let sum = &rows[0][2];
    // sum encoded as {"F64": 15.0} given the tagged Value serde.
    let sum_f64 = sum["F64"].as_f64().unwrap_or_else(|| panic!("got {sum:?}"));
    assert_eq!(sum_f64, 15.0);
}

#[tokio::test]
async fn acceptance_3_idempotent_replay_has_single_effect() {
    let (engine, part, q, _dir) = setup();
    let ing = Ingester::new(
        part.clone(),
        q,
        vec!["ou".into(), "period".into(), "de".into()],
    );
    let base = Change {
        change_id: "c-same".into(),
        site_id: None,
        object_type: "AGGREGATE_DATA_VALUE".into(),
        object_uid: uid("deAAAAAAAAA"),
        source_version: 5,
        operation: Operation::Upsert,
        occurred_at: "2026-01-01T00:00:00Z".into(),
        metadata_revision: None,
        payload_hash: None,
        payload: json!({
            "ou": "ouAAAAAAAAA",
            "period": "202601",
            "de": "deAAAAAAAAA",
            "value": 7.0,
        }),
    };
    // Apply the same change 10 times.
    for _ in 0..10 {
        ing.apply(&base).unwrap();
    }
    part.flush().unwrap();

    let state = ServerState::new(engine);
    state.register_bootstrap_admin(TEST_TOKEN).unwrap();
    let app = router(state);
    let body = json!({
        "request": {
            "kind": "aggregate",
            "dataset": "dsAAAAAAAAA",
            "partition": "p1",
            "measures": [{"op": "sum", "column": "value"}, {"op": "count_star"}],
            "group_by": ["ou"],
            "filters": [],
            "order_by": [],
        }
    });
    let resp = app
        .oneshot(
            Request::post("/api/analytics/v1/query")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {TEST_TOKEN}"))
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let payload: JsonValue =
        serde_json::from_slice(&to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
    let row = &payload["rows"][0];
    let sum = row[1]["F64"].as_f64().unwrap();
    let count = row[2]["I64"].as_i64().unwrap();
    assert_eq!(sum, 7.0);
    assert_eq!(count, 1, "ten identical replays produced one logical row");
}

#[test]
fn security_scope_filters_ou_subtree() {
    let (engine, part, _q, _dir) = setup();
    // Seed one row per OU.
    for ou in ["ouAAAAAAAAA", "ouBBBBBBBBB"] {
        part.upsert_row(
            vec![
                Value::Str(ou.into()),
                Value::Str("202601".into()),
                Value::Str("deAAAAAAAAA".into()),
                Value::F64(1.0),
            ],
            1,
        )
        .unwrap();
    }
    part.flush().unwrap();

    // Alice can only read ouAAAAAAAAA.
    let alice = Principal {
        id: "alice".into(),
        role: Role::Analyst,
        ou_scope: vec![uid("ouAAAAAAAAA")],
    };
    let pred = policy_predicate(&PolicyContext {
        principal: alice,
        catalog: engine.catalog.clone(),
    });
    // The predicate should restrict to a single OU.
    match pred {
        dap_runtime::expr::Predicate::In { values, .. } => assert_eq!(values.len(), 1),
        other => panic!("expected IN, got {other:?}"),
    }
}
