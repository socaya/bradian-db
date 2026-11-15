//! Benchmark context + report emitted alongside criterion output.
//!
//! Every published latency number must include this metadata (spec §12).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchContext {
    pub commit: String,
    pub rustc_version: String,
    pub host: String,
    pub cores: usize,
    pub rows: usize,
    pub seed: u64,
    pub concurrency: usize,
    pub cache_state: String,
    pub notes: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchReport {
    pub name: String,
    pub context: BenchContext,
    pub p50_ns: Option<u64>,
    pub p95_ns: Option<u64>,
    pub p99_ns: Option<u64>,
    pub mean_ns: Option<u64>,
}
