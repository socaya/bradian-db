//! Provenance metadata attached to every analytical result.
//!
//! Spec guiding principle 6 (§1.1): "Every result states data freshness, source versions,
//! metadata revision and consistency mode." §4 defines the four consistency modes.

use serde::{Deserialize, Serialize};

use crate::fingerprint::Fingerprint;

/// Consistency mode requested (or applied) for a query.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConsistencyMode {
    /// Return the freshest local state the edge has observed, including unacknowledged data.
    EdgeLatest,
    /// Return state the edge considers durable on local disk, excluding in-flight ingest.
    EdgeStable,
    /// Return state the central server has durably acknowledged.
    CentralAcknowledged,
    /// Return a specific snapshot timestamp (ms since epoch).
    AsOf(i64),
}

impl Default for ConsistencyMode {
    fn default() -> Self {
        ConsistencyMode::EdgeStable
    }
}

/// Coverage report per contributing site.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteCoverage {
    /// Site UIDs that fully contributed to the result.
    pub complete: Vec<String>,
    /// Site UIDs with partial contribution (e.g. stale watermark).
    pub partial: Vec<String>,
    /// Site UIDs that should have contributed but did not.
    pub missing: Vec<String>,
}

impl SiteCoverage {
    pub fn is_complete(&self) -> bool {
        self.partial.is_empty() && self.missing.is_empty()
    }
}

/// Provenance stamp attached to every `QueryResult`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// Data revision — monotonic across accepted ingestion for this partition.
    pub data_revision: u64,
    /// Catalog metadata revision used at bind time.
    pub metadata_revision: Fingerprint,
    /// Consistency mode actually applied (may differ from requested if downgraded).
    pub consistency: ConsistencyMode,
    /// Max ingest-to-visible watermark (ms since epoch) across contributing partitions.
    pub watermark_ms: i64,
    /// Site contribution breakdown.
    #[serde(default)]
    pub site_coverage: SiteCoverage,
    /// True iff the result has no estimated / approximated components.
    pub is_exact: bool,
    /// Optional note (e.g. "small-cell suppression applied").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Provenance {
    pub fn minimal(metadata_revision: Fingerprint) -> Self {
        Self {
            data_revision: 0,
            metadata_revision,
            consistency: ConsistencyMode::default(),
            watermark_ms: 0,
            site_coverage: SiteCoverage::default(),
            is_exact: true,
            note: None,
        }
    }
}
