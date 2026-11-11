//! Incremental materialized views.
//!
//! Spec §7.3 + INC-001..004. The v1 surface covers:
//!
//! 1. A [`DependencyDag`] linking source columns and data elements to downstream views.
//! 2. A [`FreshnessStamp`] classifier distinguishing fresh / data-stale / metadata-stale
//!    states.
//! 3. A [`MaterializedView`] wrapper carrying a `RecordBatch` body and its manifest.
//! 4. A [`materialize`] module that computes DHIS2 Indicator / Program Indicator values
//!    across a `(OU, period)` grid and stores the results.
#![deny(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod dag;
pub mod freshness;
pub mod materialize;
pub mod view;

pub use dag::{DependencyDag, NodeId};
pub use freshness::{FreshnessStamp, Staleness};
pub use materialize::{
    materialize_aggregate_indicator, materialize_enrollment_program_indicator,
    materialize_event_program_indicator, PiMaterialization,
};
pub use view::{MaterializedView, ViewManifest};
