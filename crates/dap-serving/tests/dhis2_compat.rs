//! Integration: /api/analytics.json emits the DHIS2 envelope shape.

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

fn seed_engine(root: &std::path::Path) -> Arc<AnalyticsEngine> {
    let cat = Catalog::new();
    cat.apply(vec![MetadataObject::Dataset(Dataset {
        uid: uid("dsAAAAAAAAA"),
        name: "DHIS2 compat test".into(),
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
    for (ou, pe, v) in [
        ("ouAAAAAAAAA", "202601", 10.0),
        ("ouAAAAAAAAA", "202602", 12.0),
        ("ouBBBBBBBBB", "202601", 20.0),
    ] {
        p.upsert_row(
            vec![
                Value::Str(ou.into()),
                Value::Str(pe.into()),
                Value::Str("deAAAAAAAAA".into()),
                Value::F64(v),
            ],
            1,
        )
        .unwrap();
    }
    p.flush().unwrap();
    engine
}

#[tokio::test]
async fn aggregate_analytics_returns_dhis2_envelope() {
    let dir = tempdir().unwrap();
    let engine = seed_engine(dir.path());
    let app = router(ServerState::new(engine));

    let body = json!({
        "dataset": "dsAAAAAAAAA",
        "partition": "p1",
        "pe": ["202601", "202602"],
        "aggregation": ["SUM"]
    });
    let resp = app
        .oneshot(
            Request::post("/api/analytics.json")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let payload: JsonValue =
        serde_json::from_slice(&to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();

    // DHIS2 envelope shape.
    assert!(payload["headers"].is_array());
    assert!(payload["rows"].is_array());
    assert!(payload["metaData"].is_object());
    assert_eq!(payload["engine"], JsonValue::Null); // engine lives under provenance
    assert_eq!(payload["provenance"]["engine"], "dap");
    assert_eq!(payload["provenance"]["is_exact"], true);

    // Headers carry DHIS2 valueType strings.
    let headers = payload["headers"].as_array().unwrap();
    assert!(headers
        .iter()
        .any(|h| h["name"] == "ou" && h["valueType"] == "TEXT"));
    assert!(headers
        .iter()
        .any(|h| h["name"] == "sum_value" && h["valueType"] == "NUMBER"));

    let width = payload["width"].as_u64().unwrap();
    let height = payload["height"].as_u64().unwrap();
    assert!(height > 0, "at least one row aggregated");
    assert!(width >= 3);
}

#[tokio::test]
async fn raw_data_endpoint_rejects_deliberately() {
    let dir = tempdir().unwrap();
    let engine = seed_engine(dir.path());
    let app = router(ServerState::new(engine));
    let resp = app
        .oneshot(
            Request::post("/api/analytics/rawData.json")
                .header("content-type", "application/json")
                .body(axum::body::Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
}

#[tokio::test]
async fn aggregate_missing_dataset_rejects_with_400() {
    let dir = tempdir().unwrap();
    let engine = seed_engine(dir.path());
    let app = router(ServerState::new(engine));
    let resp = app
        .oneshot(
            Request::post("/api/analytics.json")
                .header("content-type", "application/json")
                .body(axum::body::Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
