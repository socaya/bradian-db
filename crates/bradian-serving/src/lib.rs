//! HTTP/JSON analytical API (axum).
//!
//! Spec §10. Versioned under `/api/analytics/v1`. v1 surface: engine health, catalog,
//! aggregate query, explain, freshness. Jobs/export routes land in a later gate.
#![deny(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod auth;
pub mod dhis2_compat;
pub mod engine;
pub mod error;
pub mod jobs;
pub mod routes;
pub mod state;

pub use engine::AnalyticsEngine;
pub use jobs::{JobProgress, JobRecord, JobRegistry, JobStatus};
pub use routes::router;
pub use state::{ServerState, SharedState};
