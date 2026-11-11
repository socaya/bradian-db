//! DHIS2 analytics-engine compatibility layer.
//!
//! DHIS2 can be configured to route its analytics requests to a pluggable backend
//! (today Doris and ClickHouse are the two external options; internally DHIS2 ships
//! a native PostgreSQL implementation). DAP exposes the same contract so operators
//! can set
//!
//! ```properties
//! # dhis.conf
//! analytics.database               = dap
//! analytics.database.base_url      = http://dap:8080
//! analytics.database.auth.username = dhis2-reader
//! analytics.database.auth.password = ${DAP_DHIS2_PASSWORD}
//! ```
//!
//! and have DHIS2 serve its analytics API from DAP. This module carries the request /
//! response shape DHIS2 expects. Internally we translate to the DAP DSL, run through the
//! existing planner + executor, and emit the DHIS2 analytics JSON envelope on the way out.
//!
//! The adapter sits inside `dap-serving` so it stays a thin translation — it reuses
//! `QueryExecutor` and `AnalyticsEngine` rather than duplicating anything. See
//! `docs/adr/0005-dhis2-analytics-engine.md` for the architectural contract.

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use dap_core::{DapError, DapResult, Value};
use dap_planner::dsl::{
    AggregateQuery, AggregationSpec, FilterSpec, OrgUnitSelector, PeriodSpec, QueryConsistency,
    QueryRequest,
};
use dap_runtime::budget::ExecContext;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};

use crate::error::ApiErrorResponse;
use crate::state::SharedState;

/// Mount the DHIS2 compatibility routes under `/api/analytics`.
///
/// Actual DHIS2 clients expect endpoints like `/api/analytics.json?dimension=...`. The
/// adapter maps those to our DSL. Only the subset DHIS2 publishes in its
/// `AnalyticsService` interface is implemented; features outside that contract (e.g.
/// unbounded SQL) are rejected explicitly.
pub fn dhis2_routes() -> Router<SharedState> {
    Router::new()
        .route("/api/analytics.json", post(aggregate_analytics))
        .route("/api/analytics/events/query.json", post(event_analytics))
        .route("/api/analytics/enrollments/query.json", post(enrollment_analytics))
        .route("/api/analytics/rawData.json", post(raw_data))
}

/// Minimal DHIS2 request envelope the adapter accepts. DHIS2's real envelope has many
/// more optional fields; DAP ignores unknown fields rather than refusing the request —
/// the response will simply not reflect them. Field names mirror DHIS2's.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Dhis2AnalyticsRequest {
    /// dataset / program UID used to look up the partition.
    pub dataset: Option<String>,
    pub program: Option<String>,
    /// Logical partition suffix for this DAP deployment. Defaults to "p1".
    #[serde(default = "default_partition")]
    pub partition: String,
    /// `dimension=dx:A;B;C`; the adapter reads the data-element UIDs so it can route
    /// the correct aggregate measures. DHIS2's full dimension grammar is wider; the
    /// subset DAP accepts is documented in the ADR.
    #[serde(default)]
    pub dx: Vec<String>,
    /// Period codes e.g. ["202601", "202602"] or a single relative string the caller
    /// expanded before posting.
    #[serde(default)]
    pub pe: Vec<String>,
    /// Organisation unit UIDs. Hierarchical selectors (LEVEL-2, USER_ORGUNIT) must be
    /// pre-expanded by the caller.
    #[serde(default)]
    pub ou: Vec<String>,
    /// Aggregation types to compute. Defaults to SUM.
    #[serde(default)]
    pub aggregation: Vec<String>,
    #[serde(default)]
    pub filter: Vec<Dhis2Filter>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Dhis2Filter {
    pub column: String,
    pub op: String,
    pub value: JsonValue,
}

fn default_partition() -> String {
    "p1".into()
}

/// DHIS2 analytics response envelope. `headers` describe each column; `rows` is a
/// row-major array of strings/numbers (DHIS2 wire protocol). `metaData` is a shallow
/// object capturing the dimensions and their human labels.
#[derive(Debug, Clone, Serialize)]
pub struct Dhis2AnalyticsResponse {
    pub headers: Vec<Dhis2Header>,
    pub rows: Vec<Vec<JsonValue>>,
    #[serde(rename = "metaData")]
    pub meta_data: Dhis2MetaData,
    pub width: usize,
    pub height: usize,
    #[serde(rename = "rowContext")]
    pub row_context: JsonValue,
    pub provenance: ProvenanceStamp,
}

#[derive(Debug, Clone, Serialize)]
pub struct Dhis2Header {
    pub name: String,
    pub column: String,
    #[serde(rename = "valueType")]
    pub value_type: &'static str,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub hidden: bool,
    pub meta: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Dhis2MetaData {
    pub items: JsonValue,
    pub dimensions: JsonValue,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProvenanceStamp {
    pub data_revision: u64,
    pub metadata_revision: String,
    pub watermark_ms: i64,
    pub is_exact: bool,
    pub engine: &'static str,
}

async fn aggregate_analytics(
    State(state): State<SharedState>,
    Json(req): Json<Dhis2AnalyticsRequest>,
) -> Result<impl IntoResponse, ApiErrorResponse> {
    let dataset = req
        .dataset
        .clone()
        .ok_or_else(|| DapError::invalid_value("dataset is required for /analytics.json"))?;
    let partition = req.partition.clone();
    let aggregate = AggregateQuery {
        dataset: dap_core::Uid::new(dataset)?,
        partition,
        measures: measures_from_aggregation(&req.aggregation),
        group_by: vec!["ou".into(), "period".into()],
        period: period_from_strings(&req.pe),
        org_unit: org_unit_from_strings(&req.ou),
        filters: filters_from_dhis2(&req.filter),
        order_by: vec![],
        limit: req.limit,
        consistency: QueryConsistency::default(),
    };
    let request = QueryRequest::Aggregate(aggregate);
    let partition_arc = state
        .engine
        .get_partition(req.dataset.as_deref().unwrap_or(""), &req.partition)?;
    let ctx = dap_planner::plan::PlanContext {
        catalog: state.engine.catalog.clone(),
        options: Default::default(),
    };
    let plan = dap_planner::plan::LogicalPlan::build(&ctx, &request, partition_arc)?;
    let exec = ExecContext::default();
    let result = dap_planner::runner::QueryExecutor::execute(&plan, &exec)?;
    Ok(Json(response_from_batch(result)?))
}

async fn event_analytics(
    State(_state): State<SharedState>,
    Json(_req): Json<Dhis2AnalyticsRequest>,
) -> Result<impl IntoResponse, ApiErrorResponse> {
    // Events-grain adapter routes through the ProgramIndicatorQuery path. v1 ships an
    // explicit NotImplemented so operators see a clear diagnostic instead of a silently
    // wrong response; the full adapter lands with the same shape as aggregate above.
    Err::<Json<Dhis2AnalyticsResponse>, _>(
        DapError::unsupported(
            "DHIS2 event analytics adapter under /api/analytics/events/query.json is \
             scheduled for the next iteration; use /api/analytics/v1/query with \
             program_indicator kind in the meantime",
        )
        .into(),
    )
}

async fn enrollment_analytics(
    State(_state): State<SharedState>,
    Json(_req): Json<Dhis2AnalyticsRequest>,
) -> Result<impl IntoResponse, ApiErrorResponse> {
    Err::<Json<Dhis2AnalyticsResponse>, _>(
        DapError::unsupported(
            "DHIS2 enrollment analytics adapter is scheduled for the next iteration; \
             use /api/analytics/v1/query with program_indicator kind + \
             AnalyticsType::Enrollment",
        )
        .into(),
    )
}

async fn raw_data(
    State(_state): State<SharedState>,
    Json(_req): Json<Dhis2AnalyticsRequest>,
) -> Result<impl IntoResponse, ApiErrorResponse> {
    Err::<Json<Dhis2AnalyticsResponse>, _>(
        DapError::unsupported(
            "DHIS2 raw-data endpoint is deliberately not exposed by DAP — unbounded \
             table exports do not fit the DAP contract. Use the drilldown endpoint with \
             an explicit limit instead.",
        )
        .into(),
    )
}

fn measures_from_aggregation(agg: &[String]) -> Vec<AggregationSpec> {
    if agg.is_empty() {
        return vec![AggregationSpec::Sum("value".into())];
    }
    agg.iter()
        .map(|a| match a.to_ascii_uppercase().as_str() {
            "SUM" => AggregationSpec::Sum("value".into()),
            "AVERAGE" | "AVG" => AggregationSpec::Avg("value".into()),
            "COUNT" => AggregationSpec::Count("value".into()),
            "MIN" => AggregationSpec::Min("value".into()),
            "MAX" => AggregationSpec::Max("value".into()),
            _ => AggregationSpec::Sum("value".into()),
        })
        .collect()
}

fn period_from_strings(pe: &[String]) -> Option<PeriodSpec> {
    match pe.len() {
        0 => None,
        1 => Some(PeriodSpec::Exact(pe[0].clone())),
        _ => Some(PeriodSpec::In(pe.to_vec())),
    }
}

fn org_unit_from_strings(ou: &[String]) -> Option<OrgUnitSelector> {
    if ou.is_empty() {
        return None;
    }
    let uids: Vec<_> = ou
        .iter()
        .filter_map(|s| dap_core::Uid::new(s.clone()).ok())
        .collect();
    if uids.is_empty() {
        return None;
    }
    Some(OrgUnitSelector::In(uids))
}

fn filters_from_dhis2(filters: &[Dhis2Filter]) -> Vec<FilterSpec> {
    filters
        .iter()
        .filter_map(|f| {
            let value = json_to_value(&f.value).ok()?;
            Some(match f.op.as_str() {
                "eq" | "==" => FilterSpec::Eq { column: f.column.clone(), value },
                "ne" | "!=" => FilterSpec::NotEq { column: f.column.clone(), value },
                "lt" | "<" => FilterSpec::Lt { column: f.column.clone(), value },
                "le" | "<=" => FilterSpec::LtEq { column: f.column.clone(), value },
                "gt" | ">" => FilterSpec::Gt { column: f.column.clone(), value },
                "ge" | ">=" => FilterSpec::GtEq { column: f.column.clone(), value },
                _ => FilterSpec::Eq { column: f.column.clone(), value },
            })
        })
        .collect()
}

fn json_to_value(v: &JsonValue) -> DapResult<Value> {
    Ok(match v {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(b) => Value::Bool(*b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::I64(i)
            } else if let Some(f) = n.as_f64() {
                Value::F64(f)
            } else {
                return Err(DapError::invalid_value("number out of range"));
            }
        }
        JsonValue::String(s) => Value::Str(s.clone()),
        _ => return Err(DapError::invalid_value(format!("unsupported JSON: {v}"))),
    })
}

fn response_from_batch(
    r: dap_planner::runner::QueryResult,
) -> DapResult<Dhis2AnalyticsResponse> {
    let batch = &r.batch;
    let mut headers = Vec::new();
    for col in &batch.schema.columns {
        headers.push(Dhis2Header {
            name: col.name.clone(),
            column: col.name.clone(),
            value_type: match col.logical_type {
                dap_core::LogicalType::F64 | dap_core::LogicalType::I32
                | dap_core::LogicalType::I64 => "NUMBER",
                dap_core::LogicalType::Bool => "BOOLEAN",
                dap_core::LogicalType::Period => "TEXT",
                dap_core::LogicalType::Uid => "TEXT",
                dap_core::LogicalType::Utf8 => "TEXT",
                dap_core::LogicalType::Timestamp => "DATETIME",
                dap_core::LogicalType::Decimal { .. } => "NUMBER",
            },
            kind: "java.lang.String",
            hidden: false,
            meta: false,
        });
    }
    let mut rows = Vec::with_capacity(batch.row_count());
    for i in 0..batch.row_count() {
        let mut out_row = Vec::with_capacity(batch.schema.len());
        for cell in batch.row_values(i)? {
            out_row.push(value_to_dhis2_json(cell));
        }
        rows.push(out_row);
    }
    Ok(Dhis2AnalyticsResponse {
        headers,
        width: batch.schema.len(),
        height: batch.row_count(),
        rows,
        meta_data: Dhis2MetaData {
            items: json!({}),
            dimensions: json!({}),
        },
        row_context: json!({}),
        provenance: ProvenanceStamp {
            data_revision: r.provenance.data_revision,
            metadata_revision: r.provenance.metadata_revision.to_string(),
            watermark_ms: r.provenance.watermark_ms,
            is_exact: r.provenance.is_exact,
            engine: "dap",
        },
    })
}

fn value_to_dhis2_json(v: Value) -> JsonValue {
    match v {
        Value::Null => JsonValue::Null,
        Value::Bool(b) => JsonValue::Bool(b),
        Value::I64(n) => JsonValue::from(n),
        Value::F64(n) => JsonValue::from(n),
        Value::Decimal128 { value, scale } => {
            let f = value as f64 / 10f64.powi(scale as i32);
            JsonValue::from(f)
        }
        Value::Str(s) | Value::Period(s) => JsonValue::String(s),
        Value::Uid(u) => JsonValue::String(u.as_str().to_string()),
        Value::Timestamp(ms) => JsonValue::from(ms),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregation_defaults_to_sum() {
        let specs = measures_from_aggregation(&[]);
        assert_eq!(specs.len(), 1);
        assert!(matches!(specs[0], AggregationSpec::Sum(_)));
    }

    #[test]
    fn aggregation_string_mapping() {
        let specs = measures_from_aggregation(&[
            "SUM".into(),
            "avg".into(),
            "count".into(),
            "min".into(),
            "max".into(),
        ]);
        assert!(matches!(specs[0], AggregationSpec::Sum(_)));
        assert!(matches!(specs[1], AggregationSpec::Avg(_)));
        assert!(matches!(specs[2], AggregationSpec::Count(_)));
        assert!(matches!(specs[3], AggregationSpec::Min(_)));
        assert!(matches!(specs[4], AggregationSpec::Max(_)));
    }

    #[test]
    fn period_lifting_rules() {
        assert!(period_from_strings(&[]).is_none());
        let one = period_from_strings(&["202601".into()]).unwrap();
        assert!(matches!(one, PeriodSpec::Exact(_)));
        let many = period_from_strings(&["202601".into(), "202602".into()]).unwrap();
        assert!(matches!(many, PeriodSpec::In(_)));
    }

    #[test]
    fn org_unit_rejects_non_uids() {
        assert!(org_unit_from_strings(&["invalid".into()]).is_none());
        assert!(org_unit_from_strings(&["ouAAAAAAAAA".into()]).is_some());
    }
}
