//! DER Semantic Query DSL — the JSON contract exposed on the serving API.
//!
//! Spec §6.1 (1). The DSL binds to metadata by UID (never by display label), and compiles
//! into the same logical AST as the SQL subset. v1 covers aggregate analytical queries:
//! SUM / COUNT / COUNT DISTINCT / MIN / MAX / AVG grouped by any combination of OU / period /
//! data element / category combinations, filtered by period range, OU subtree and arbitrary
//! equality / comparison predicates.

use dap_core::{ConsistencyMode, PeriodType, Uid, Value};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum QueryRequest {
    /// Aggregate analytical query over a dataset partition.
    Aggregate(AggregateQuery),
    /// Program Indicator evaluation over an event/tracker program.
    ProgramIndicator(ProgramIndicatorQuery),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProgramIndicatorQuery {
    pub program: Uid,
    pub partition: String,
    /// DHIS2 program indicator UID. The catalog must hold its compiled form.
    pub indicator: Uid,
    /// Group the per-grain results by one or more OU UIDs (sub-tree expansion is done by
    /// the planner when `org_unit_subtree` is set).
    #[serde(default)]
    pub group_by: Vec<String>,
    #[serde(default)]
    pub period: Option<PeriodSpec>,
    #[serde(default)]
    pub org_unit: Option<OrgUnitSelector>,
    #[serde(default)]
    pub filters: Vec<FilterSpec>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub consistency: QueryConsistency,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AggregateQuery {
    pub dataset: Uid,
    pub partition: String,
    pub measures: Vec<AggregationSpec>,
    #[serde(default)]
    pub group_by: Vec<String>,
    #[serde(default)]
    pub period: Option<PeriodSpec>,
    #[serde(default)]
    pub org_unit: Option<OrgUnitSelector>,
    #[serde(default)]
    pub filters: Vec<FilterSpec>,
    #[serde(default)]
    pub order_by: Vec<OrderSpec>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub consistency: QueryConsistency,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "op", content = "column")]
pub enum AggregationSpec {
    CountStar,
    Count(String),
    CountDistinct(String),
    Sum(String),
    Min(String),
    Max(String),
    Avg(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeriodSpec {
    /// Match periods of a specific DHIS2 type within an inclusive [start_code, end_code] range.
    /// Both codes must be of the specified `period_type`.
    Range {
        period_type: PeriodType,
        start: String,
        end: String,
    },
    /// Match exactly one period code.
    Exact(String),
    /// Match any of the listed codes.
    In(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrgUnitSelector {
    /// Exactly these org-unit UIDs.
    In(Vec<Uid>),
    /// All descendants of each listed root OU, inclusive.
    Subtree(Vec<Uid>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "op")]
pub enum FilterSpec {
    Eq { column: String, value: Value },
    NotEq { column: String, value: Value },
    Lt { column: String, value: Value },
    LtEq { column: String, value: Value },
    Gt { column: String, value: Value },
    GtEq { column: String, value: Value },
    In { column: String, values: Vec<Value> },
    IsNull { column: String },
    IsNotNull { column: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrderSpec {
    pub column: String,
    #[serde(default)]
    pub descending: bool,
    #[serde(default = "nulls_last_default")]
    pub nulls_first: bool,
}

const fn nulls_last_default() -> bool {
    false
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct QueryConsistency {
    #[serde(default)]
    pub mode: Option<SerdeConsistencyMode>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SerdeConsistencyMode {
    EdgeLatest,
    EdgeStable,
    CentralAcknowledged,
}

impl From<SerdeConsistencyMode> for ConsistencyMode {
    fn from(m: SerdeConsistencyMode) -> Self {
        match m {
            SerdeConsistencyMode::EdgeLatest => ConsistencyMode::EdgeLatest,
            SerdeConsistencyMode::EdgeStable => ConsistencyMode::EdgeStable,
            SerdeConsistencyMode::CentralAcknowledged => ConsistencyMode::CentralAcknowledged,
        }
    }
}
