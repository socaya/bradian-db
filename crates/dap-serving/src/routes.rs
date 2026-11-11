//! HTTP routes under `/api/analytics/v1`.

use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::{Path, State};
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

/// Canonical API router. Mounts the DAP analytical endpoints plus the DHIS2
/// compatibility routes. Add the admin UI with [`router_with_static_ui`].
pub fn router(state: SharedState) -> Router {
    api_router(state)
}

pub fn api_router(state: SharedState) -> Router {
    Router::new()
        .route("/api/analytics/v1/engine/health", get(engine_health))
        .route("/api/analytics/v1/catalog", get(catalog_summary))
        .route("/api/analytics/v1/freshness", post(freshness))
        .route("/api/analytics/v1/query", post(run_query))
        .route("/api/analytics/v1/query/explain", post(explain_query))
        .route("/api/analytics/v1/drilldown", post(drilldown))
        .route("/api/analytics/v1/ingest", post(ingest_batch))
        .route("/api/analytics/v1/data-products", get(list_data_products).post(register_data_product))
        .route("/api/analytics/v1/data-products/:id", get(get_data_product).delete(delete_data_product))
        .route("/api/analytics/v1/jobs", get(list_jobs).post(submit_async_query))
        .route("/api/analytics/v1/jobs/:id", get(get_job))
        .route("/api/analytics/v1/jobs/:id/cancel", post(cancel_job))
        .route("/healthz", get(healthz))
        .merge(crate::dhis2_compat::dhis2_routes())
        .with_state(state)
}

/// Serve the API alongside a built static UI. `ui_dir` must contain `index.html` plus
/// Vite build output. Non-API paths resolve to `index.html` so SPA routing works.
pub fn router_with_static_ui(state: SharedState, ui_dir: std::path::PathBuf) -> Router {
    let index = ui_dir.join("index.html");
    let fallback = tower_http::services::ServeFile::new(index);
    let static_service = tower_http::services::ServeDir::new(ui_dir)
        .append_index_html_on_directories(true)
        .fallback(fallback);
    api_router(state).fallback_service(static_service)
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
    let data_revision = segments.iter().map(|s| s.data_revision).max().unwrap_or(0);
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

#[derive(Clone, Serialize)]
pub(crate) struct QueryEnvelopeResponse {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub provenance: dap_core::Provenance,
    pub metrics: dap_runtime::budget::Snapshot,
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
    #[serde(default)]
    pub drill_filters: Vec<dap_planner::dsl::FilterSpec>,
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

/// Registered analytical data product. Published alongside a materialised view.
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

async fn get_data_product(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErrorResponse> {
    state
        .data_products
        .read()
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or_else(|| dap_core::DapError::NotFound(format!("data product {id}")).into())
}

async fn delete_data_product(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErrorResponse> {
    let removed = state.data_products.write().remove(&id);
    if removed.is_none() {
        return Err(dap_core::DapError::NotFound(format!("data product {id}")).into());
    }
    Ok(StatusCode::NO_CONTENT)
}

// ── Bulk ingest (used by the DHIS2 analytics-table rebuild) ─────────────────

#[derive(Serialize, Deserialize, Debug)]
pub struct IngestBatchRequest {
    pub dataset: String,
    pub partition: String,
    pub changes: Vec<dap_ingest::Change>,
}

#[derive(Serialize, Debug)]
pub struct IngestBatchResponse {
    pub applied: u64,
    pub deletes: u64,
    pub dropped_stale: u64,
    pub quarantined: u64,
}

async fn ingest_batch(
    State(state): State<SharedState>,
    Json(body): Json<IngestBatchRequest>,
) -> Result<impl IntoResponse, ApiErrorResponse> {
    // Reach for the matching partition. Operators point DHIS2 at a dataset+partition
    // pair they created through the admin CLI; the ingest endpoint refuses unknown
    // partitions rather than silently creating one (STO-005 version fences live on the
    // partition, not on the ingest envelope).
    let partition = state.engine.get_partition(&body.dataset, &body.partition)?;
    let pk = partition
        .schema()
        .columns
        .iter()
        .take_while(|c| c.name != "value")
        .map(|c| c.name.clone())
        .collect::<Vec<_>>();
    let quarantine = dap_ingest::QuarantineLog::open(partition.dir())?;
    let ingester = dap_ingest::Ingester::new(partition.clone(), quarantine, pk);
    let report = ingester.apply_batch(&body.changes)?;
    Ok(Json(IngestBatchResponse {
        applied: report.accepted,
        deletes: report.deletes,
        dropped_stale: report.dropped_stale,
        quarantined: report.quarantined,
    }))
}

// ── Async jobs (API-003) ────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug)]
struct SubmitJobRequest {
    pub request: QueryRequest,
    #[serde(default)]
    pub submitted_by: Option<String>,
    /// Hard cap on the number of output rows the job may stash in its `result` payload.
    /// Jobs that produce more fail with a precise diagnostic instead of landing a
    /// pathologically large JSON blob in the registry. See [`DEFAULT_MAX_EXPORT_ROWS`].
    #[serde(default)]
    pub max_output_rows: Option<usize>,
}

/// Default cap on async-query output size — enough for the typical analytics export,
/// small enough that a misconfigured client cannot OOM the engine via the job registry.
pub const DEFAULT_MAX_EXPORT_ROWS: usize = 100_000;

async fn submit_async_query(
    State(state): State<SharedState>,
    Json(body): Json<SubmitJobRequest>,
) -> Result<impl IntoResponse, ApiErrorResponse> {
    let job_id = format!("job-{}", uuid_like());
    let state_clone = state.clone();
    let request = body.request.clone();
    let cap = body.max_output_rows.unwrap_or(DEFAULT_MAX_EXPORT_ROWS);
    let record = state.jobs.submit(
        job_id.clone(),
        "query",
        body.submitted_by,
        move |_cancel, progress| async move {
            progress.set(json!({"step": "planning", "max_output_rows": cap}));
            let response = tokio::task::spawn_blocking(move || {
                execute_request(&state_clone, &request)
            })
            .await
            .map_err(|e| dap_core::DapError::other(format!("spawn_blocking: {e}")))??;
            if response.rows.len() > cap {
                return Err(dap_core::DapError::invalid_value(format!(
                    "async query produced {} rows, exceeds max_output_rows={cap}; \
                     narrow the query or raise max_output_rows",
                    response.rows.len()
                )));
            }
            progress.set(json!({"step": "complete", "output_rows": response.metrics.output_rows}));
            serde_json::to_value(response).map_err(dap_core::DapError::from)
        },
    );
    Ok((StatusCode::ACCEPTED, Json(record)))
}

async fn list_jobs(State(state): State<SharedState>) -> impl IntoResponse {
    Json(state.jobs.list())
}

async fn get_job(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErrorResponse> {
    state
        .jobs
        .get(&id)
        .map(Json)
        .ok_or_else(|| dap_core::DapError::NotFound(format!("job {id}")).into())
}

async fn cancel_job(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErrorResponse> {
    state.jobs.cancel(&id)?;
    Ok(Json(json!({"id": id, "status": "CANCELLED"})))
}

fn uuid_like() -> String {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let salt = std::process::id() as u128;
    let digest = blake3::hash(&(ns ^ salt).to_le_bytes());
    let hex = digest.to_hex();
    hex[..16].to_string()
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

pub(crate) fn execute_request(
    state: &SharedState,
    request: &QueryRequest,
) -> DapResult<QueryEnvelopeResponse> {
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
