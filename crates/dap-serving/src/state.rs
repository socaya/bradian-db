//! Shared state carried by every handler.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use dap_security::{Principal, Role, TokenStore};

use crate::engine::AnalyticsEngine;
use crate::jobs::JobRegistry;
use crate::routes::DataProduct;

#[derive(Debug)]
pub struct ServerState {
    pub engine: Arc<AnalyticsEngine>,
    pub query_history: RwLock<Vec<QueryHistoryEntry>>,
    pub data_products: RwLock<HashMap<String, DataProduct>>,
    pub jobs: Arc<JobRegistry>,
    pub api_tokens: TokenStore,
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
    /// Provision a local bootstrap administrator. Not a production identity provider.
    pub fn register_bootstrap_admin(&self, token: &str) -> Result<(), &'static str> {
        if token.len() < 32 || token.chars().any(char::is_whitespace) {
            return Err("bootstrap token must contain at least 32 non-whitespace characters");
        }
        self.api_tokens.register(token, Principal {
            id: "bootstrap-admin".into(),
            role: Role::Administrator,
            ou_scope: vec![],
        });
        Ok(())
    }

    pub fn new(engine: Arc<AnalyticsEngine>) -> SharedState {
        Arc::new(Self {
            engine,
            query_history: RwLock::new(Vec::new()),
            data_products: RwLock::new(HashMap::new()),
            jobs: JobRegistry::new(),
            api_tokens: TokenStore::new(),
        })
    }
}
