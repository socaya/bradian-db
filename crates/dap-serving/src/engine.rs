//! `AnalyticsEngine` composes the catalog + a set of open partitions. One engine per server
//! process. Partitions keyed by `(dataset_id, partition_id)`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use dap_catalog::Catalog;
use dap_core::{DapError, DapResult};
use dap_storage::{Partition, PartitionOptions};
use parking_lot::RwLock;

#[derive(Debug)]
pub struct AnalyticsEngine {
    pub root: PathBuf,
    pub catalog: Arc<Catalog>,
    partitions: RwLock<HashMap<String, Arc<Partition>>>,
}

impl AnalyticsEngine {
    pub fn new(root: PathBuf, catalog: Arc<Catalog>) -> Self {
        Self {
            root,
            catalog,
            partitions: RwLock::new(HashMap::new()),
        }
    }

    pub fn key(dataset: &str, partition: &str) -> String {
        format!("{dataset}:{partition}")
    }

    pub fn open_partition(&self, options: PartitionOptions) -> DapResult<Arc<Partition>> {
        let key = Self::key(&options.dataset_id, &options.partition_id);
        if let Some(existing) = self.partitions.read().get(&key).cloned() {
            return Ok(existing);
        }
        let part = Partition::open(&self.root, options)?;
        self.partitions.write().insert(key, part.clone());
        Ok(part)
    }

    pub fn get_partition(&self, dataset: &str, partition: &str) -> DapResult<Arc<Partition>> {
        let key = Self::key(dataset, partition);
        self.partitions
            .read()
            .get(&key)
            .cloned()
            .ok_or_else(|| DapError::NotFound(format!("partition {key} not open")))
    }

    pub fn list_partitions(&self) -> Vec<(String, String)> {
        self.partitions
            .read()
            .values()
            .map(|p| (p.dir().to_string_lossy().to_string(), String::new()))
            .collect()
    }
}
