//! Worker registry and executor trait.

use std::sync::Arc;

use dap_core::{DapError, DapResult};
use dap_planner::plan::LogicalPlan;
use dap_runtime::budget::ExecContext;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::fragment::{Fragment, FragmentResult};

/// Plan-loader shorthand to keep the clippy `type_complexity` lint happy.
pub type PlanLoader = Arc<dyn Fn(&Fragment) -> DapResult<LogicalPlan> + Send + Sync>;

/// Stable worker identifier. In practice this is a DHIS2 site UID, a cluster-node UID or
/// a short label for a test harness.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkerId(pub String);

impl WorkerId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Capability declaration published by the worker at registration time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerCapabilities {
    /// Dataset UIDs this worker can answer.
    pub datasets: Vec<String>,
    /// Program UIDs this worker can answer.
    pub programs: Vec<String>,
    /// Max rows the worker is willing to return per fragment.
    pub max_fragment_rows: u64,
    /// Whether the worker can execute cross-site joins.
    pub supports_cross_site_join: bool,
    /// Human-readable location label (used by residency policy).
    pub location: String,
}

/// Trait every worker implements. The coordinator calls `execute` once per assigned
/// fragment. The execution is synchronous here; the admin app wires a reqwest-based remote
/// executor on top for cross-process workers.
pub trait FragmentExecutor: Send + Sync {
    fn id(&self) -> &WorkerId;
    fn capabilities(&self) -> &WorkerCapabilities;
    fn execute(&self, fragment: &Fragment, ctx: &ExecContext) -> DapResult<FragmentResult>;
}

/// In-process worker that executes a fragment through the local `QueryExecutor`. Tests use
/// this directly; a production edge wraps its partition index + catalog into one.
pub struct InProcessWorker {
    id: WorkerId,
    caps: WorkerCapabilities,
    plan_loader: PlanLoader,
}

impl InProcessWorker {
    pub fn new(id: WorkerId, caps: WorkerCapabilities, plan_loader: PlanLoader) -> Self {
        Self {
            id,
            caps,
            plan_loader,
        }
    }
}

impl FragmentExecutor for InProcessWorker {
    fn id(&self) -> &WorkerId {
        &self.id
    }
    fn capabilities(&self) -> &WorkerCapabilities {
        &self.caps
    }
    fn execute(&self, fragment: &Fragment, ctx: &ExecContext) -> DapResult<FragmentResult> {
        let plan = (self.plan_loader)(fragment)?;
        let q = dap_planner::runner::QueryExecutor::execute(&plan, ctx)?;
        if q.batch.row_count() as u64 > self.caps.max_fragment_rows {
            return Err(DapError::BudgetExceeded(format!(
                "fragment emitted {} rows, limit is {}",
                q.batch.row_count(),
                self.caps.max_fragment_rows
            )));
        }
        Ok(FragmentResult {
            worker_id: self.id.clone(),
            fragment_id: fragment.id.clone(),
            batch: q.batch,
            provenance: q.provenance,
        })
    }
}

impl std::fmt::Debug for InProcessWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InProcessWorker")
            .field("id", &self.id)
            .field("caps", &self.caps)
            .finish()
    }
}

#[derive(Default)]
pub struct WorkerRegistry {
    workers: RwLock<Vec<Arc<dyn FragmentExecutor>>>,
}

impl std::fmt::Debug for WorkerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ids: Vec<_> = self
            .workers
            .read()
            .iter()
            .map(|w| w.id().as_str().to_string())
            .collect();
        f.debug_struct("WorkerRegistry").field("workers", &ids).finish()
    }
}

impl WorkerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, worker: Arc<dyn FragmentExecutor>) {
        self.workers.write().push(worker);
    }

    pub fn find_for_dataset(&self, dataset: &str) -> Vec<Arc<dyn FragmentExecutor>> {
        self.workers
            .read()
            .iter()
            .filter(|w| {
                w.capabilities()
                    .datasets
                    .iter()
                    .any(|d| d == dataset)
            })
            .cloned()
            .collect()
    }

    pub fn find_for_program(&self, program: &str) -> Vec<Arc<dyn FragmentExecutor>> {
        self.workers
            .read()
            .iter()
            .filter(|w| {
                w.capabilities()
                    .programs
                    .iter()
                    .any(|p| p == program)
            })
            .cloned()
            .collect()
    }

    pub fn all(&self) -> Vec<Arc<dyn FragmentExecutor>> {
        self.workers.read().clone()
    }

    pub fn len(&self) -> usize {
        self.workers.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.workers.read().is_empty()
    }
}
