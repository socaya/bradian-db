//! Freshness tracking for materialized views.

use dap_core::Fingerprint;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshnessStamp {
    /// Max watermark observed across upstream partitions (ms since epoch).
    pub upstream_watermark_ms: i64,
    /// Catalog revision the view was last materialized against.
    pub metadata_revision: Fingerprint,
    /// Last successful refresh (ms since epoch).
    pub last_refresh_ms: i64,
    /// Rows in the current materialization.
    pub rows: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Staleness {
    Fresh,
    /// Underlying data has moved forward but the view has not been refreshed.
    DataStale,
    /// Catalog metadata has changed; the view must be rebuilt (not incrementally refreshed).
    MetadataStale,
    Unknown,
}

impl FreshnessStamp {
    pub fn classify(&self, current_watermark_ms: i64, current_metadata: Fingerprint) -> Staleness {
        if self.metadata_revision != current_metadata {
            return Staleness::MetadataStale;
        }
        if current_watermark_ms > self.upstream_watermark_ms {
            return Staleness::DataStale;
        }
        Staleness::Fresh
    }
}
