//! HTTP-level integration test using tower's `oneshot`.

use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Request, StatusCode};
use dap_catalog::metadata::Dataset;
use dap_catalog::{Catalog, MetadataObject};
use dap_core::{Column, LogicalType, PeriodType, Schema, Uid, Value};
use dap_serving::{router, AnalyticsEngine, ServerState};
use dap_storage::PartitionOptions;
use serde_json::{json, Value as JsonValue};
use tempfile::tempdir;
use tower::ServiceExt;

fn uid(s: &str) -> Uid {
    Uid::new(s).unwrap()
}

fn seed(root: &std::path::Path) -> Arc<dap_serving::AnalyticsEngine> {
    let cat = Catalog::new();
    cat.apply(vec![MetadataObject::Dataset(Dataset {
        uid: uid("dsAAAAAAAAA"),
        name: "Agg Dataset".into(),
        period_type: PeriodType::Monthly,
        data_elements: vec![uid("deAAAAAAAAA")],
    })])
    .unwrap();

    let engine = Arc::new(AnalyticsEngine::new(root.to_path_buf(), Arc::new(cat)));

    let schema = Schema::new(vec![
        Column::new("ou", LogicalType::Uid).required(),
        Column::new("period", LogicalType::Period).required(),
        Column::new("de", LogicalType::Uid).required(),
        Column::new("value", LogicalType::F64),
    ])
    .unwrap();
    let p = engine
        .open_partition(PartitionOptions {
            dataset_id: "dsAAAAAAAAA".into(),
            partition_id: "p1".into(),
            schema,
            primary_key_columns: vec!["ou".into(), "period".into(), "de".into()],
            flush_rows: 1024,
            flush_bytes: 1 << 24,
        })
        .unwrap();

    for (ou, per, v) in &[
        ("ouAAAAAAAAA", "202601", 10.0),
        ("ouBBBBBBBBB", "202601", 20.0),
        ("ouAAAAAAAAA", "202602", 12.0),
    ] {
        p.upsert_row(
            vec![
                Value::Str((*ou).into()),
                Value::Str((*per).into()),
                Value::Str("deAAAAAAAAA".into()),
                Value::F64(*v),
            ],
            1,
        )
        .unwrap();
    }
    p.flush().unwrap();

    engine
}

#[tokio::test]
async fn healthz_returns_ok() {
    let dir = tempdir().unwrap();
    let engine = seed(dir.path());
    let app = router(ServerState::new(engine));

    let resp = app
        .oneshot(Request::get("/healthz").body(axum::body::Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn query_endpoint_returns_rows_with_provenance() {
    let dir = tempdir().unwrap();
    let engine = seed(dir.path());
    let app = router(ServerState::new(engine));

    let body = json!({
        "request": {
            "kind": "aggregate",
            "dataset": "dsAAAAAAAAA",
            "partition": "p1",
            "measures": [{"op": "sum", "column": "value"}],
            "group_by": ["ou"],
            "period": {"range": {"period_type": "MONTHLY", "start": "202601", "end": "202602"}},
            "filters": [],
            "order_by": [],
        }
    });

    let resp = app
        .oneshot(
            Request::post("/api/analytics/v1/query")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "body: {:?}", resp);

    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let payload: JsonValue = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(payload["columns"][0], "ou");
    assert_eq!(payload["columns"][1], "sum_value");
    let rows = payload["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);

    // Provenance stamp.
    assert!(payload["provenance"]["metadata_revision"].is_string());
    assert!(payload["provenance"]["is_exact"].as_bool().unwrap_or(false));
    assert!(payload["metrics"]["scanned_rows"].as_u64().unwrap() >= 3);
}

#[tokio::test]
async fn explain_endpoint_returns_operator_tree() {
    let dir = tempdir().unwrap();
    let engine = seed(dir.path());
    let app = router(ServerState::new(engine));

    let body = json!({
        "request": {
            "kind": "aggregate",
            "dataset": "dsAAAAAAAAA",
            "partition": "p1",
            "measures": [{"op": "count_star"}],
            "group_by": ["period"],
            "filters": [],
            "order_by": [],
        }
    });

    let resp = app
        .oneshot(
            Request::post("/api/analytics/v1/query/explain")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let payload: JsonValue = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(payload["root"]["name"], "HashAggregate");
    assert_eq!(
        payload["root"]["children"][0]["name"], "Filter",
        "expected Filter under HashAggregate"
    );
}
