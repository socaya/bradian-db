//! Query executor — drive runtime operators according to the (physical) plan.

use dap_core::{ConsistencyMode, DapResult, Provenance, SiteCoverage};
use dap_runtime::aggregate::HashAggregator;
use dap_runtime::batch::{RecordBatch, RecordBatchBuilder};
use dap_runtime::budget::ExecContext;
use dap_runtime::scan::{scan_partition, ScanOptions};
#[allow(unused_imports)]
use dap_runtime::aggregate::AggregateKind as _AggKind;
use dap_runtime::sort::sort_batch;

use crate::plan::LogicalPlan;

#[derive(Clone, Debug)]
pub struct QueryResult {
    pub batch: RecordBatch,
    pub provenance: Provenance,
    pub metrics: dap_runtime::budget::Snapshot,
}

#[derive(Debug, Default)]
pub struct QueryExecutor;

impl QueryExecutor {
    pub fn execute(plan: &LogicalPlan, ctx: &ExecContext) -> DapResult<QueryResult> {
        // Scan without projection in v1 so the aggregator input schema matches the
        // partition schema. Projection pruning is a later gate (OPT-001).
        let batches = scan_partition(&plan.partition, &ScanOptions::default(), ctx)?;

        // Filter + feed into hash aggregator.
        let mut agg = HashAggregator::new(
            plan.partition.schema().clone(),
            plan.group_by.clone(),
            plan.aggregates.clone(),
        )?;
        for b in &batches {
            let selection = plan.predicate.eval_batch(b)?;
            if selection.is_empty() {
                continue;
            }
            let mut filtered = RecordBatchBuilder::new(b.schema.clone());
            for r in selection {
                filtered.append_row(&b.row_values(r as usize)?)?;
            }
            agg.consume_batch(&filtered.finish()?, ctx)?;
        }
        let mut result = agg.finish(ctx)?;

        // Sort + limit (v1 in-memory).
        if !plan.order_by.is_empty() {
            result = sort_batch(&result, &plan.order_by)?;
        }
        if let Some(limit) = plan.limit {
            if result.row_count() > limit {
                let mut b = RecordBatchBuilder::new(result.schema.clone());
                for r in 0..limit {
                    b.append_row(&result.row_values(r)?)?;
                }
                result = b.finish()?;
            }
        }

        // Provenance.
        let provenance = Provenance {
            data_revision: plan
                .partition
                .visible_segments()
                .iter()
                .map(|s| s.data_revision)
                .max()
                .unwrap_or(0),
            metadata_revision: plan.partition.schema_fingerprint(),
            consistency: ConsistencyMode::default(),
            watermark_ms: plan
                .partition
                .visible_segments()
                .iter()
                .map(|s| s.watermark_ms)
                .max()
                .unwrap_or(0),
            site_coverage: SiteCoverage::default(),
            // v1: every aggregate is exact (COUNT DISTINCT uses a BTreeSet oracle, not a
            // sketch). This flag flips once approximate sketches land under OPT-003.
            is_exact: true,
            note: None,
        };
        Ok(QueryResult {
            batch: result,
            provenance,
            metrics: ctx.metrics.snapshot(),
        })
    }

}

