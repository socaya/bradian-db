//! Materialized view manifest. v1 stores the full result as a single in-memory
//! `RecordBatch` plus its freshness stamp. Persistence to `.derc` segments and incremental
//! refresh are deferred to a later gate.

use dap_runtime::batch::RecordBatch;

use crate::freshness::FreshnessStamp;

#[derive(Clone, Debug)]
pub struct ViewManifest {
    pub id: String,
    pub owner: String,
    pub grain: Vec<String>,
    pub measures: Vec<String>,
    pub consumers: Vec<String>,
    pub freshness: FreshnessStamp,
}

#[derive(Clone, Debug)]
pub struct MaterializedView {
    pub manifest: ViewManifest,
    pub body: RecordBatch,
}

impl MaterializedView {
    pub fn new(manifest: ViewManifest, body: RecordBatch) -> Self {
        Self { manifest, body }
    }

    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub fn body(&self) -> &RecordBatch {
        &self.body
    }
}
