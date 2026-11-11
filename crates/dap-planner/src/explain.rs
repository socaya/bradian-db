//! EXPLAIN output. Produces a rendered tree of operators + estimated row counts.

use serde::{Deserialize, Serialize};

use crate::plan::LogicalPlan;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExplainPlan {
    pub root: OperatorNode,
    pub partition: String,
    pub dataset: String,
    pub metadata_revision: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperatorNode {
    pub name: String,
    pub details: String,
    #[serde(default)]
    pub estimated_rows: Option<u64>,
    #[serde(default)]
    pub children: Vec<OperatorNode>,
}

pub fn explain(plan: &LogicalPlan, metadata_revision: &str) -> ExplainPlan {
    let partition = plan.partition.visible_segments();
    let total_rows: u64 = partition.iter().map(|s| s.row_count).sum();

    let mut root = OperatorNode {
        name: "Scan".into(),
        details: format!(
            "segments={}, row_count={}",
            partition.len(),
            total_rows
        ),
        estimated_rows: Some(total_rows),
        children: vec![],
    };
    root = OperatorNode {
        name: "Filter".into(),
        details: format!("{:?}", plan.predicate),
        estimated_rows: Some(total_rows / 2),
        children: vec![root],
    };
    root = OperatorNode {
        name: "HashAggregate".into(),
        details: format!(
            "group_by={:?}, aggs={}",
            plan.group_by,
            plan.aggregates.len()
        ),
        estimated_rows: None,
        children: vec![root],
    };
    if !plan.order_by.is_empty() {
        root = OperatorNode {
            name: "Sort".into(),
            details: format!("keys={}", plan.order_by.len()),
            estimated_rows: None,
            children: vec![root],
        };
    }
    if let Some(lim) = plan.limit {
        root = OperatorNode {
            name: "Limit".into(),
            details: format!("limit={lim}"),
            estimated_rows: Some(lim as u64),
            children: vec![root],
        };
    }
    ExplainPlan {
        root,
        partition: plan
            .partition
            .visible_segments()
            .first()
            .map(|s| s.segment_id.clone())
            .unwrap_or_else(|| "(empty)".into()),
        dataset: plan.partition.schema_fingerprint().to_string(),
        metadata_revision: metadata_revision.to_string(),
    }
}
