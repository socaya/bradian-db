//! Async job registry (API-003).
//!
//! The query API serves bounded synchronous responses. Long-running work — bulk exports,
//! materialised-view rebuilds, benchmark runs — is submitted as a Job and polled until it
//! reaches a terminal state. Jobs survive handler scope but not process restart: that
//! durability layer plugs in later, keyed on a job-journal segment.

use std::collections::HashMap;
use std::sync::Arc;

use dap_core::{DapError, DapResult};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JobStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub kind: String,
    pub submitted_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
    pub status: JobStatus,
    /// Opaque progress payload (JSON) the job may update while running.
    #[serde(default)]
    pub progress: serde_json::Value,
    /// Final result payload (set on Succeeded). Typically small — large exports stream
    /// through a separate `/exports/<id>` route that reads a spill file.
    #[serde(default)]
    pub result: serde_json::Value,
    /// Error diagnostic (set on Failed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Principal id that submitted the job. Used for audit + revocation checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_by: Option<String>,
}

impl JobRecord {
    pub fn new(id: String, kind: String, submitted_by: Option<String>) -> Self {
        Self {
            id,
            kind,
            submitted_at_ms: now_ms(),
            started_at_ms: None,
            finished_at_ms: None,
            status: JobStatus::Pending,
            progress: serde_json::Value::Null,
            result: serde_json::Value::Null,
            error: None,
            submitted_by,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled
        )
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Default)]
pub struct JobRegistry {
    jobs: RwLock<HashMap<String, JobRecord>>,
    handles: RwLock<HashMap<String, JobHandle>>,
}

#[derive(Debug)]
struct JobHandle {
    cancel: Arc<std::sync::atomic::AtomicBool>,
    /// Retained so the spawned task is not detached from the registry's lifetime; aborting
    /// via `handle.abort()` is a future extension.
    _task: Option<JoinHandle<()>>,
}

impl JobRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Submit an async job. The closure receives (`cancel`, `progress_setter`) and returns
    /// a `serde_json::Value` on success or a `DapError`. The runtime spawns the task on
    /// the ambient tokio runtime; callers pass through from an axum handler.
    pub fn submit<F, Fut>(
        self: &Arc<Self>,
        id: String,
        kind: impl Into<String>,
        submitted_by: Option<String>,
        work: F,
    ) -> JobRecord
    where
        F: FnOnce(Arc<std::sync::atomic::AtomicBool>, Arc<JobProgress>) -> Fut
            + Send
            + 'static,
        Fut: std::future::Future<Output = DapResult<serde_json::Value>> + Send + 'static,
    {
        let record = JobRecord::new(id.clone(), kind.into(), submitted_by);
        self.jobs.write().insert(id.clone(), record.clone());

        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let progress = Arc::new(JobProgress::new(id.clone(), self.clone()));

        let registry = self.clone();
        let cancel_clone = cancel.clone();
        let progress_clone = progress.clone();
        let id_task = id.clone();
        let task = tokio::spawn(async move {
            registry.update(&id_task, |r| {
                r.status = JobStatus::Running;
                r.started_at_ms = Some(now_ms());
            });
            let outcome = work(cancel_clone.clone(), progress_clone).await;
            registry.update(&id_task, |r| {
                r.finished_at_ms = Some(now_ms());
                if cancel_clone.load(std::sync::atomic::Ordering::Relaxed) {
                    r.status = JobStatus::Cancelled;
                    return;
                }
                match outcome {
                    Ok(v) => {
                        r.status = JobStatus::Succeeded;
                        r.result = v;
                    }
                    Err(e) => {
                        r.status = JobStatus::Failed;
                        r.error = Some(e.to_string());
                    }
                }
            });
        });
        self.handles.write().insert(
            id,
            JobHandle {
                cancel,
                _task: Some(task),
            },
        );
        record
    }

    pub fn get(&self, id: &str) -> Option<JobRecord> {
        self.jobs.read().get(id).cloned()
    }

    pub fn list(&self) -> Vec<JobRecord> {
        let mut out: Vec<_> = self.jobs.read().values().cloned().collect();
        out.sort_by_key(|r| r.submitted_at_ms);
        out
    }

    pub fn cancel(&self, id: &str) -> DapResult<()> {
        let handle = self
            .handles
            .read()
            .get(id)
            .map(|h| h.cancel.clone())
            .ok_or_else(|| DapError::NotFound(format!("job {id}")))?;
        handle.store(true, std::sync::atomic::Ordering::Relaxed);
        self.update(id, |r| {
            if !r.is_terminal() {
                r.status = JobStatus::Cancelled;
                r.finished_at_ms = Some(now_ms());
            }
        });
        Ok(())
    }

    fn update(&self, id: &str, mutate: impl FnOnce(&mut JobRecord)) {
        if let Some(record) = self.jobs.write().get_mut(id) {
            mutate(record);
        }
    }
}

/// Handle a running job uses to report progress.
#[derive(Debug)]
pub struct JobProgress {
    id: String,
    registry: Arc<JobRegistry>,
}

impl JobProgress {
    fn new(id: String, registry: Arc<JobRegistry>) -> Self {
        Self { id, registry }
    }

    pub fn set(&self, payload: serde_json::Value) {
        self.registry.update(&self.id, |r| {
            r.progress = payload;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn submit_runs_and_completes() {
        let reg = JobRegistry::new();
        let _ = reg.submit(
            "job1".into(),
            "test",
            None,
            |_cancel, progress| async move {
                progress.set(serde_json::json!({"step": "working"}));
                Ok(serde_json::json!({"ok": true}))
            },
        );
        // Wait for the task to drain.
        for _ in 0..100 {
            if let Some(r) = reg.get("job1") {
                if r.is_terminal() {
                    assert_eq!(r.status, JobStatus::Succeeded);
                    assert_eq!(r.result["ok"], true);
                    return;
                }
            }
            tokio::task::yield_now().await;
        }
        panic!("job did not terminate");
    }

    #[tokio::test]
    async fn cancellation_marks_job_cancelled() {
        let reg = JobRegistry::new();
        let _ = reg.submit(
            "job2".into(),
            "long",
            None,
            |cancel, _progress| async move {
                while !cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    tokio::task::yield_now().await;
                }
                Ok(serde_json::json!({}))
            },
        );
        reg.cancel("job2").unwrap();
        for _ in 0..100 {
            if let Some(r) = reg.get("job2") {
                if r.is_terminal() {
                    assert_eq!(r.status, JobStatus::Cancelled);
                    return;
                }
            }
            tokio::task::yield_now().await;
        }
        panic!("cancellation did not reach terminal state");
    }

    #[tokio::test]
    async fn failed_job_carries_error() {
        let reg = JobRegistry::new();
        let _ = reg.submit("job3".into(), "boom", None, |_cancel, _p| async move {
            Err(DapError::other("explicit failure"))
        });
        for _ in 0..100 {
            if let Some(r) = reg.get("job3") {
                if r.is_terminal() {
                    assert_eq!(r.status, JobStatus::Failed);
                    assert!(r.error.unwrap().contains("explicit failure"));
                    return;
                }
            }
            tokio::task::yield_now().await;
        }
        panic!("failure did not propagate");
    }
}
