//! Quarantine log for invalid payloads. Append-only JSON Lines under
//! `<analytics_root>/quarantine/<yyyy-mm>.log`.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use dap_core::DapResult;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuarantinedChange {
    pub change_id: String,
    pub reason: String,
    pub payload: serde_json::Value,
    pub received_at_ms: i64,
}

#[derive(Debug)]
pub struct QuarantineLog {
    path: PathBuf,
    lock: Mutex<()>,
}

impl QuarantineLog {
    pub fn open(root: &std::path::Path) -> DapResult<Self> {
        let dir = root.join("quarantine");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("quarantine.log");
        Ok(Self {
            path,
            lock: Mutex::new(()),
        })
    }

    pub fn append(&self, q: &QuarantinedChange) -> DapResult<()> {
        let _g = self.lock.lock();
        let mut f = OpenOptions::new().create(true).append(true).open(&self.path)?;
        let line = serde_json::to_string(q)?;
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")?;
        f.flush()?;
        Ok(())
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}
