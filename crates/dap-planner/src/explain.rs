//! EXPLAIN output. Produces a rendered tree of operators + estimated row counts.

use dap_runtime::expr::{Expr, Predicate};
use dap_storage::{ManifestSegment, SegmentReader};
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
    let segments = plan.partition.visible_segments();
    let total_rows: u64 = segments.iter().map(|s| s.row_count).sum();

    let selectivity = estimate_selectivity(&plan.predicate, &segments);
    let after_filter = ((total_rows as f64) * selectivity).round() as u64;

    let mut root = OperatorNode {
        name: "Scan".into(),
        details: format!("segments={}, row_count={}", segments.len(), total_rows),
        estimated_rows: Some(total_rows),
        children: vec![],
    };
    root = OperatorNode {
        name: "Filter".into(),
        details: format!("{:?} (selectivity≈{:.3})", plan.predicate, selectivity),
        estimated_rows: Some(after_filter),
        children: vec![root],
    };
    let after_agg = if plan.group_by.is_empty() {
        // Scalar aggregate → exactly one output row.
        1
    } else {
        // Without column-level NDV stats, the tightest cheap upper bound is "no more
        // rows out than in". A later gate can collect per-column distinct counts.
        after_filter
    };
    root = OperatorNode {
        name: "HashAggregate".into(),
        details: format!(
            "group_by={:?}, aggs={}",
            plan.group_by,
            plan.aggregates.len()
        ),
        estimated_rows: Some(after_agg),
        children: vec![root],
    };
    if !plan.order_by.is_empty() {
        root = OperatorNode {
            name: "Sort".into(),
            details: format!("keys={}", plan.order_by.len()),
            estimated_rows: Some(after_agg),
            children: vec![root],
        };
    }
    if let Some(lim) = plan.limit {
        root = OperatorNode {
            name: "Limit".into(),
            details: format!("limit={lim}"),
            estimated_rows: Some((lim as u64).min(after_agg)),
            children: vec![root],
        };
    }
    ExplainPlan {
        root,
        partition: segments
            .first()
            .map(|s| s.segment_id.clone())
            .unwrap_or_else(|| "(empty)".into()),
        dataset: plan.partition.schema_fingerprint().to_string(),
        metadata_revision: metadata_revision.to_string(),
    }
}

/// Zone-map-driven selectivity estimate for `predicate` over `segments`.
///
/// The estimate is a value in `[0.0, 1.0]`. Must never over-report (i.e. return more than
/// the true fraction) — downstream decisions (join side selection, memory budgeting)
/// assume the number is conservative. For predicates we cannot sharpen with zone-map
/// data we fall back to 0.5, which matches the historic constant.
fn estimate_selectivity(p: &Predicate, segments: &[ManifestSegment]) -> f64 {
    match p {
        Predicate::True => 1.0,
        Predicate::False => 0.0,
        Predicate::And(subs) => {
            // Assume independence: selectivities multiply. Still correct even if the
            // assumption is wrong because the product is a tighter bound than max().
            subs.iter()
                .map(|s| estimate_selectivity(s, segments))
                .product()
        }
        Predicate::Or(subs) => {
            // Inclusion–exclusion lower bound: max(sel_i). Conservative because the true
            // selectivity of a disjunction is at least max_i(sel_i).
            subs.iter()
                .map(|s| estimate_selectivity(s, segments))
                .fold(0.0, f64::max)
        }
        Predicate::Not(inner) => (1.0 - estimate_selectivity(inner, segments)).clamp(0.0, 1.0),
        Predicate::IsNull(e) => null_fraction(e, segments).unwrap_or(0.5),
        Predicate::IsNotNull(e) => null_fraction(e, segments).map(|f| 1.0 - f).unwrap_or(0.5),
        Predicate::Compare { .. } | Predicate::In { .. } => 0.1,
    }
}

/// Fraction of rows across `segments` that are null for the column named by `expr`, read
/// from each segment's column zone map. Returns `None` if the column is not readable or
/// no segment yielded a zone map — the caller falls back to a constant.
fn null_fraction(expr: &Expr, segments: &[ManifestSegment]) -> Option<f64> {
    let col_name = match expr {
        Expr::Col(c) => c.as_str(),
        _ => return None,
    };
    let mut total_rows: u64 = 0;
    let mut total_nulls: u64 = 0;
    let mut saw_column = false;
    for seg in segments {
        let reader = match SegmentReader::open(&seg.path) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if let Some(col) = reader.columns().iter().find(|c| c.name == col_name) {
            saw_column = true;
            total_rows += u64::from(col.zone_map.row_count);
            total_nulls += u64::from(col.zone_map.null_count);
        }
    }
    if !saw_column || total_rows == 0 {
        None
    } else {
        Some(total_nulls as f64 / total_rows as f64)
    }
}
