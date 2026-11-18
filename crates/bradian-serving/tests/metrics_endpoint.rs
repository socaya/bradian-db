//! ADR 0169 — HTTP /metrics endpoint integration test.

use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Request, StatusCode};
use bradian_catalog::Catalog;
use bradian_serving::{router, AnalyticsEngine, ServerState};
use tempfile::tempdir;
use tower::ServiceExt;

fn make_state() -> bradian_serving::state::SharedState {
    let dir = tempdir().unwrap();
    let engine = Arc::new(AnalyticsEngine::new(
        dir.path().to_path_buf(), Arc::new(Catalog::new())));
    std::mem::forget(dir);  // keep the tempdir alive for the test
    ServerState::new(engine)
}

#[tokio::test]
async fn metrics_endpoint_is_public_and_serves_text_plain() {
    let state = make_state();
    let app = router(state.clone());
    let response = app.oneshot(
        Request::builder().uri("/metrics")
            .body(axum::body::Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let ct = response.headers()
        .get(axum::http::header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap_or("").to_string())
        .unwrap_or_default();
    assert!(ct.starts_with("text/plain"));
}

#[tokio::test]
async fn metrics_endpoint_renders_registered_counters() {
    let state = make_state();
    state.metrics.counter("sql.queries").add(7);
    let app = router(state.clone());
    let response = app.oneshot(
        Request::builder().uri("/metrics")
            .body(axum::body::Body::empty()).unwrap()
    ).await.unwrap();
    let body = to_bytes(response.into_body(), 10 * 1024).await.unwrap();
    let text = std::str::from_utf8(&body).unwrap();
    assert!(text.contains("# TYPE sql_queries counter"));
    assert!(text.contains("sql_queries 7"));
}

#[tokio::test]
async fn metrics_endpoint_is_reachable_without_auth_token() {
    let state = make_state();
    let app = router(state);
    // Note: no Authorization header provided.
    let response = app.oneshot(
        Request::builder().uri("/metrics")
            .body(axum::body::Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn protected_analytics_endpoint_still_requires_auth() {
    // Sanity check — we haven't accidentally made all
    // routes public.
    let state = make_state();
    let app = router(state);
    let response = app.oneshot(
        Request::builder().uri("/api/analytics/v1/catalog")
            .body(axum::body::Body::empty()).unwrap()
    ).await.unwrap();
    assert_ne!(response.status(), StatusCode::OK);
}
