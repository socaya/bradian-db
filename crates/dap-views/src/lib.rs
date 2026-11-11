//! Incremental materialized views.
//!
//! Spec §7.3 + INC-001..004. v1 scope is a small scaffold: a dependency DAG that links
//! source columns to computed measures, plus an invalidator that marks downstream views
//! stale when the catalog revision changes or a delta tombstone fires. Fully incremental
//! retractions ship in a later gate once the planner can push them through hash aggregates.
#![deny(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod dag;
pub mod freshness;
pub mod view;

pub use dag::{DependencyDag, NodeId};
pub use freshness::{FreshnessStamp, Staleness};
pub use view::{MaterializedView, ViewManifest};
