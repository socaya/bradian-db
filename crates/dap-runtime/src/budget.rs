//! Per-query resource budgets and metrics.
//!
//! Spec §5.3 ("every query has estimated and hard budgets for memory, CPU, scan bytes, …"),
//! §12 workload classes, requirement RUN-003.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use dap_core::{DapError, DapResult};

#[derive(Clone, Debug)]
pub struct MemoryBudget {
    /// Hard ceiling in bytes. 0 means unlimited.
    pub hard_limit: u64,
    /// Soft threshold that triggers spill-to-disk once implemented. Must be <= hard_limit.
    pub spill_threshold: u64,
}

impl MemoryBudget {
    pub const fn unlimited() -> Self {
        Self { hard_limit: 0, spill_threshold: 0 }
    }
    pub const fn limited(hard_limit: u64) -> Self {
        Self {
            hard_limit,
            spill_threshold: hard_limit / 2,
        }
    }
}

impl Default for MemoryBudget {
    fn default() -> Self {
        Self::unlimited()
    }
}

#[derive(Debug, Default)]
pub struct RuntimeMetrics {
    pub scanned_rows: AtomicU64,
    pub scanned_bytes: AtomicU64,
    pub output_rows: AtomicU64,
    pub peak_memory_bytes: AtomicU64,
    pub spilled_bytes: AtomicU64,
}

impl RuntimeMetrics {
    pub fn record_scan(&self, rows: u64, bytes: u64) {
        self.scanned_rows.fetch_add(rows, Ordering::Relaxed);
        self.scanned_bytes.fetch_add(bytes, Ordering::Relaxed);
    }
    pub fn record_output(&self, rows: u64) {
        self.output_rows.fetch_add(rows, Ordering::Relaxed);
    }
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            scanned_rows: self.scanned_rows.load(Ordering::Relaxed),
            scanned_bytes: self.scanned_bytes.load(Ordering::Relaxed),
            output_rows: self.output_rows.load(Ordering::Relaxed),
            peak_memory_bytes: self.peak_memory_bytes.load(Ordering::Relaxed),
            spilled_bytes: self.spilled_bytes.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    pub scanned_rows: u64,
    pub scanned_bytes: u64,
    pub output_rows: u64,
    pub peak_memory_bytes: u64,
    pub spilled_bytes: u64,
}

/// Execution context — threaded through every operator. Carries budget, cancellation and
/// metrics so operators can be composed without sharing globals.
#[derive(Clone, Debug)]
pub struct ExecContext {
    pub memory: MemoryBudget,
    pub metrics: Arc<RuntimeMetrics>,
    pub cancel: Arc<AtomicBool>,
    /// Batch width hint; operators may emit smaller batches at EOF.
    pub batch_rows: usize,
}

impl Default for ExecContext {
    fn default() -> Self {
        Self {
            memory: MemoryBudget::default(),
            metrics: Arc::new(RuntimeMetrics::default()),
            cancel: Arc::new(AtomicBool::new(false)),
            batch_rows: 4096,
        }
    }
}

impl ExecContext {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn check_cancellation(&self) -> DapResult<()> {
        if self.cancelled() {
            Err(DapError::Cancelled)
        } else {
            Ok(())
        }
    }

    pub fn account_memory(&self, delta_bytes: u64) -> DapResult<()> {
        let prev = self
            .metrics
            .peak_memory_bytes
            .fetch_add(delta_bytes, Ordering::Relaxed);
        let now = prev + delta_bytes;
        if self.memory.hard_limit > 0 && now > self.memory.hard_limit {
            Err(DapError::BudgetExceeded(format!(
                "memory budget exceeded: {now} > {}",
                self.memory.hard_limit
            )))
        } else {
            Ok(())
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
