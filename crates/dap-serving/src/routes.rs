//! HTTP routes under `/api/analytics/v1`.

use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use dap_core::{DapResult, Value};
use dap_planner::dsl::QueryRequest;
use dap_planner::plan::{LogicalPlan, PlanContext, PlanOptions};
use dap_planner::runner::QueryExecutor;
use dap_runtime::budget::ExecContext;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::ApiErrorResponse;
use crate::state::{QueryHistoryEntry, SharedState};

static QUERY_SEQ: AtomicU64 = AtomicU64::new(1);

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/analytics/v1/engine/health", get(engine_health))
        .route("/api/analytics/v1/catalog", get(catalog_summary))
        .route("/api/analytics/v1/freshness", get(freshness))
        .route("/api/analytics/v1/query", post(run_query))
        .route("/api/analytics/v1/query/explain", post(explain_query))
        .route("/api/analytics/v1/drilldown", post(drilldown))
        .route("/api/analytics/v1/data-products", get(list_data_products))
        .route("/api/analytics/v1/data-products", post(register_data_product))
        .route("/healthz", get(healthz))
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

async fn engine_health(State(state): State<SharedState>) -> impl IntoResponse {
    let partitions = state.engine.list_partitions();
    Json(json!({
        "status": "ok",
        "catalog_revision": state.engine.catalog.revision().to_string(),
        "partitions_open": partitions.len(),
        "query_history_size": state.query_history.read().len(),
    }))
}

async fn catalog_summary(State(state): State<SharedState>) -> impl IntoResponse {
    Json(json!({
        "revision": state.engine.catalog.revision().to_string(),
    }))
}

#[derive(Serialize, Deserialize)]
struct FreshnessParams {
    dataset: String,
    partition: String,
}

async fn freshness(
    State(state): State<SharedState>,
    Json(params): Json<FreshnessParams>,
) -> Result<impl IntoResponse, ApiErrorResponse> {
    let partition = state.engine.get_partition(&params.dataset, &params.partition)?;
    let segments = partition.visible_segments();
    let max_watermark = segments.iter().map(|s| s.watermark_ms).max().unwrap_or(0);
    let data_revision = segments
        .iter()
        .map(|s| s.data_revision)
        .max()
        .unwrap_or(0);
    Ok(Json(json!({
        "dataset": params.dataset,
        "partition": params.partition,
        "watermark_ms": max_watermark,
        "data_revision": data_revision,
        "segment_count": segments.len(),
    })))
}

#[derive(Serialize, Deserialize, Debug)]
struct QueryEnvelope {
    pub request: QueryRequest,
}

#[derive(Serialize)]
struct QueryEnvelopeResponse {
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
    provenance: dap_core::Provenance,
    metrics: dap_runtime::budget::Snapshot,
}

async fn run_query(
    State(state): State<SharedState>,
    Json(envelope): Json<QueryEnvelope>,
) -> Result<impl IntoResponse, ApiErrorResponse> {
    let started = std::time::Instant::now();
    let started_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let response = execute_request(&state, &envelope.request)?;
    let duration_ms = started.elapsed().as_millis() as u64;
    let id = QUERY_SEQ.fetch_add(1, Ordering::Relaxed);
    state.query_history.write().push(QueryHistoryEntry {
        id,
        started_at_ms: started_ms,
        duration_ms,
        scanned_rows: response.metrics.scanned_rows,
        output_rows: response.metrics.output_rows,
    });
    Ok(Json(response))
}

async fn explain_query(
    State(state): State<SharedState>,
    Json(envelope): Json<QueryEnvelope>,
) -> Result<impl IntoResponse, ApiErrorResponse> {
    let plan = build_plan(&state, &envelope.request)?;
    let rev = state.engine.catalog.revision().to_string();
    let plan_doc = dap_planner::explain::explain(&plan, &rev);
    Ok(Json(plan_doc))
}

#[derive(Serialize, Deserialize, Debug)]
struct DrilldownRequest {
    pub request: QueryRequest,
    /// Additional filters applied on top of the base request before execution. Typical
    /// usage: the caller clicked a cell in the aggregate output and wants a bounded
    /// line-listing-like result for that cell.
    #[serde(default)]
    pub drill_filters: Vec<dap_planner::dsl::FilterSpec>,
    /// Max rows to return. Caps at 10 000 to keep the endpoint bounded.
    #[serde(default = "default_drill_limit")]
    pub limit: usize,
}

fn default_drill_limit() -> usize {
    1_000
}

async fn drilldown(
    State(state): State<SharedState>,
    Json(body): Json<DrilldownRequest>,
) -> Result<impl IntoResponse, ApiErrorResponse> {
    let mut request = body.request;
    match &mut request {
        QueryRequest::Aggregate(aq) => {
            aq.filters.extend(body.drill_filters);
            aq.limit = Some(body.limit.min(10_000));
        }
        QueryRequest::ProgramIndicator(piq) => {
            piq.filters.extend(body.drill_filters);
            piq.limit = Some(body.limit.min(10_000));
        }
    }
    let response = execute_request(&state, &request)?;
    Ok(Json(response))
}

/// A registered analytical data product (spec §8.5). Consumers discover products through
/// `GET /data-products`; operators register them through `POST /data-products` after
/// publishing the underlying materialised view.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DataProduct {
    pub id: String,
    pub owner: String,
    pub grain: Vec<String>,
    pub measures: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub consumers: Vec<String>,
    pub freshness_contract: FreshnessContract,
    #[serde(default)]
    pub sample_query: Option<QueryRequest>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FreshnessContract {
    /// SLA for data staleness in seconds. A consumer asking for a product with a tighter
    /// SLA than the engine can meet gets back an explicit `partial` provenance note.
    pub max_staleness_seconds: u64,
    pub consistency: dap_core::ConsistencyMode,
}

async fn list_data_products(State(state): State<SharedState>) -> impl IntoResponse {
    let catalogue = state.data_products.read();
    let out: Vec<DataProduct> = catalogue.values().cloned().collect();
    Json(out)
}

async fn register_data_product(
    State(state): State<SharedState>,
    Json(product): Json<DataProduct>,
) -> Result<impl IntoResponse, ApiErrorResponse> {
    if product.id.is_empty() {
        return Err(dap_core::DapError::invalid_value("data product id is required").into());
    }
    state
        .data_products
        .write()
        .insert(product.id.clone(), product.clone());
    Ok(Json(product))
}

fn build_plan(state: &SharedState, request: &QueryRequest) -> DapResult<LogicalPlan> {
    let (dataset, partition_id) = match request {
        QueryRequest::Aggregate(aq) => (aq.dataset.as_str().to_string(), aq.partition.clone()),
        QueryRequest::ProgramIndicator(piq) => {
            (piq.program.as_str().to_string(), piq.partition.clone())
        }
    };
    let partition = state.engine.get_partition(&dataset, &partition_id)?;
    let ctx = PlanContext {
        catalog: state.engine.catalog.clone(),
        options: PlanOptions::default(),
    };
    LogicalPlan::build(&ctx, request, partition)
}

fn execute_request(state: &SharedState, request: &QueryRequest) -> DapResult<QueryEnvelopeResponse> {
    let plan = build_plan(state, request)?;
    let exec = ExecContext::default();
    let result = QueryExecutor::execute(&plan, &exec)?;

    let columns = result
        .batch
        .schema
        .columns
        .iter()
        .map(|c| c.name.clone())
        .collect::<Vec<_>>();
    let mut rows = Vec::with_capacity(result.batch.row_count());
    for r in 0..result.batch.row_count() {
        rows.push(result.batch.row_values(r)?);
    }
    Ok(QueryEnvelopeResponse {
        columns,
        rows,
        provenance: result.provenance,
        metrics: result.metrics,
    })
}
