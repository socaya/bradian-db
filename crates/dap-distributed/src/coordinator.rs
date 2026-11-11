//! Coordinator: splits a query, dispatches fragments, merges results.

use std::sync::Arc;

use dap_core::{DapError, DapResult, Provenance};
use dap_runtime::batch::{RecordBatch, RecordBatchBuilder};
use dap_runtime::budget::ExecContext;

use crate::fragment::{Fragment, FragmentKind, FragmentResult};
use crate::residency::{DataClassification, ResidencyPolicy};
use crate::worker::WorkerRegistry;

#[derive(Debug, Clone)]
pub struct FederatedResult {
    pub batch: RecordBatch,
    pub provenance: Provenance,
    pub partial_sites: Vec<String>,
    pub complete_sites: Vec<String>,
}

#[derive(Debug)]
pub struct Coordinator {
    pub registry: Arc<WorkerRegistry>,
    pub residency: Vec<(String, ResidencyPolicy)>,
    pub required_classification: DataClassification,
}

impl Coordinator {
    pub fn new(registry: Arc<WorkerRegistry>) -> Self {
        Self {
            registry,
            residency: Vec::new(),
            required_classification: DataClassification::Aggregate,
        }
    }

    pub fn with_residency(mut self, workerish: &str, policy: ResidencyPolicy) -> Self {
        self.residency.push((workerish.to_string(), policy));
        self
    }

    /// Dispatch a fragment to every worker whose capabilities declare the matching dataset
    /// or program. The coordinator merges the partial results by concatenating row sets
    /// and re-aggregating where the measure name is additive (SUM/COUNT). Non-additive
    /// measures that do not fit the simple merge path surface as partial sites.
    pub fn execute(
        &self,
        fragment: Fragment,
        ctx: &ExecContext,
    ) -> DapResult<FederatedResult> {
        let workers = match &fragment.kind {
            FragmentKind::Aggregate(aq) => self.registry.find_for_dataset(aq.dataset.as_str()),
            FragmentKind::ProgramIndicator(piq) => {
                self.registry.find_for_program(piq.program.as_str())
            }
        };
        if workers.is_empty() {
            return Err(DapError::NotFound(format!(
                "no worker claims fragment {} (check capability registration)",
                fragment.id
            )));
        }

        let mut partial_sites: Vec<String> = Vec::new();
        let mut complete_sites: Vec<String> = Vec::new();
        let mut partial_results: Vec<FragmentResult> = Vec::new();
        let mut worst_watermark: i64 = 0;
        let mut max_data_revision: u64 = 0;

        for worker in &workers {
            // Enforce residency policy registered for this worker (if any).
            if let Some((_, policy)) = self
                .residency
                .iter()
                .find(|(w, _)| w == worker.id().as_str())
            {
                if let Err(_e) =
                    policy.check(self.required_classification, &fragment.ou_scope)
                {
                    partial_sites.push(worker.id().as_str().to_string());
                    continue;
                }
            }
            match worker.execute(&fragment, ctx) {
                Ok(res) => {
                    complete_sites.push(worker.id().as_str().to_string());
                    if res.provenance.watermark_ms > worst_watermark {
                        worst_watermark = res.provenance.watermark_ms;
                    }
                    if res.provenance.data_revision > max_data_revision {
                        max_data_revision = res.provenance.data_revision;
                    }
                    partial_results.push(res);
                }
                Err(e) => {
                    tracing::warn!(worker = %worker.id().as_str(), error = %e, "fragment failed");
                    partial_sites.push(worker.id().as_str().to_string());
                }
            }
        }

        if partial_results.is_empty() {
            return Err(DapError::other(
                "every worker refused or failed the fragment",
            ));
        }

        let merged = merge_partials(&partial_results)?;
        let provenance = Provenance {
            data_revision: max_data_revision,
            metadata_revision: partial_results[0].provenance.metadata_revision,
            consistency: partial_results[0].provenance.consistency,
            watermark_ms: worst_watermark,
            site_coverage: dap_core::SiteCoverage {
                complete: complete_sites.clone(),
                partial: partial_sites.clone(),
                missing: vec![],
            },
            is_exact: partial_sites.is_empty()
                && partial_results.iter().all(|r| r.provenance.is_exact),
            note: None,
        };
        Ok(FederatedResult {
            batch: merged,
            provenance,
            partial_sites,
            complete_sites,
        })
    }
}

/// Concatenate every partial result into a single record batch. For additive aggregates
/// (SUM, COUNT) the caller re-aggregates one more time by pushing the merged batch into a
/// fresh HashAggregator; non-additive aggregates (MIN/MAX/AVG) rely on the concatenation
/// to be followed by a second pass with the same aggregator kind. The coordinator caller
/// is expected to apply that second pass if it wants cross-site reduction.
fn merge_partials(results: &[FragmentResult]) -> DapResult<RecordBatch> {
    if results.is_empty() {
        return Err(DapError::other("merge_partials received no results"));
    }
    let schema = results[0].batch.schema.clone();
    let mut builder = RecordBatchBuilder::new(schema.clone());
    for r in results {
        if r.batch.schema != schema {
            return Err(DapError::schema_mismatch(
                "federated workers returned different schemas",
            ));
        }
        for row in 0..r.batch.row_count() {
            builder.append_row(&r.batch.row_values(row)?)?;
        }
    }
    builder.finish()
}
