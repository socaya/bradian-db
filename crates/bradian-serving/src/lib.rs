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
/// ADR 0283 + ADR 0284 — rustls-backed TLS and optional mTLS.
pub mod tls;
/// ADR 0314 — Studio SQL session skeleton.
pub mod sql_session;
/// ADR 0316 — relational wire protocol + reference driver skeleton.
pub mod wire;

pub use engine::AnalyticsEngine;
pub use jobs::{JobProgress, JobRecord, JobRegistry, JobStatus};
pub use routes::router;
pub use state::{ServerState, SharedState};
pub use tls::{
    TlsAcceptorState, TlsError, TlsMaterial, TlsResult,
    load_pem, load_pem_with_client_ca,
};
