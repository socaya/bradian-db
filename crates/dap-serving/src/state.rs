//! Shared state carried by every handler.

use std::sync::Arc;

use parking_lot::RwLock;

use crate::engine::AnalyticsEngine;

#[derive(Debug)]
pub struct ServerState {
    pub engine: Arc<AnalyticsEngine>,
    pub query_history: RwLock<Vec<QueryHistoryEntry>>,
}

#[derive(Clone, Debug)]
pub struct QueryHistoryEntry {
    pub id: u64,
    pub started_at_ms: i64,
    pub duration_ms: u64,
    pub scanned_rows: u64,
    pub output_rows: u64,
}

pub type SharedState = Arc<ServerState>;

impl ServerState {
    pub fn new(engine: Arc<AnalyticsEngine>) -> SharedState {
        Arc::new(Self {
            engine,
            query_history: RwLock::new(Vec::new()),
        })
    }
}
