//! Fragment definitions used by the coordinator ↔ worker protocol.

use dap_core::Provenance;
use dap_planner::dsl::{AggregateQuery, ProgramIndicatorQuery};
use dap_runtime::batch::RecordBatch;
use serde::{Deserialize, Serialize};

use crate::worker::WorkerId;

/// The subset of query shapes that can be split across workers today. Unknown shapes
/// route to the authority that owns them without fragmentation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FragmentKind {
    /// Aggregate query with a subset of the OU set. The coordinator shards the OU subtree
    /// across the available workers and each produces partial state for its shard.
    Aggregate(AggregateQuery),
    /// Program indicator query over a program subset, same sharding model as Aggregate.
    ProgramIndicator(ProgramIndicatorQuery),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fragment {
    pub id: String,
    pub kind: FragmentKind,
    /// Residency note: the coordinator stamps the OU-scope here so a worker can reject a
    /// fragment that reaches outside its site. Empty = unrestricted.
    #[serde(default)]
    pub ou_scope: Vec<dap_core::Uid>,
}

#[derive(Clone, Debug)]
pub struct FragmentResult {
    pub worker_id: WorkerId,
    pub fragment_id: String,
    pub batch: RecordBatch,
    pub provenance: Provenance,
}
