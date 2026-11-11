//! HTTP/JSON analytical API (axum).
//!
//! Spec §10. Versioned under `/api/analytics/v1`. v1 surface: engine health, catalog,
//! aggregate query, explain, freshness. Jobs/export routes land in a later gate.
#![deny(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod engine;
pub mod error;
pub mod routes;
pub mod state;

pub use engine::AnalyticsEngine;
pub use routes::router;
pub use state::{ServerState, SharedState};
