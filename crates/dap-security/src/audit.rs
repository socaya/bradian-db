//! Append-only audit log. JSON Lines for operator ease.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use dap_core::DapResult;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp_ms: i64,
    pub principal_id: String,
    pub action: String,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug)]
pub struct AuditLog {
    path: PathBuf,
    lock: Mutex<()>,
}

impl AuditLog {
    pub fn open(path: PathBuf) -> DapResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(Self {
            path,
            lock: Mutex::new(()),
        })
    }

    pub fn record(&self, entry: &AuditEntry) -> DapResult<()> {
        let _g = self.lock.lock();
        let mut f = OpenOptions::new().create(true).append(true).open(&self.path)?;
        let line = serde_json::to_string(entry)?;
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")?;
        Ok(())
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn append_single_entry() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = AuditLog::open(path.clone()).unwrap();
        log.record(&AuditEntry {
            timestamp_ms: 1,
            principal_id: "alice".into(),
            action: "query".into(),
            target: "ds/p".into(),
            plan_fingerprint: Some("abc".into()),
            detail: None,
        })
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.ends_with(b"\n"));
        assert!(std::str::from_utf8(&bytes).unwrap().contains("alice"));
    }
}
