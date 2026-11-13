//! Query, backing-map, retained-history and staging limits; not a total RSS reservation.
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use thiserror::Error;

#[derive(Clone, Copy, Debug)]
pub struct ScanLimits {
    pub max_rows: usize,
    pub max_bytes: usize,
    pub max_work: usize,
}
impl Default for ScanLimits {
    fn default() -> Self { Self { max_rows: 100000, max_bytes: 16 * 1024 * 1024, max_work: 1000000 } }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ResourceError {
    #[error("index build {resource} limit exceeded (limit {limit})")]
    IndexBuildLimit { resource: &'static str, limit: usize },
    #[error("index recovery {resource} limit exceeded (limit {limit})")]
    IndexLimit { resource: &'static str, limit: usize },
    #[error("row store {resource} limit exceeded (limit {limit})")]
    RowStoreLimit { resource: &'static str, limit: usize },
    #[error("retained history {resource} limit exceeded (limit {limit})")]
    HistoryLimit { resource: &'static str, limit: usize },
    #[error("transaction staging {resource} limit exceeded (limit {limit})")]
    StagingLimit { resource: &'static str, limit: usize },
    #[error("query cancelled")]
    Cancelled,
    #[error("query {resource} budget exceeded (limit {limit})")]
    Limit { resource: &'static str, limit: usize },
}

#[derive(Clone, Debug)]
pub struct ScanControl {
    limits: ScanLimits,
    cancelled: Arc<AtomicBool>,
}
impl Default for ScanControl { fn default() -> Self { Self::new(ScanLimits::default()) } }
impl ScanControl {
    pub fn new(limits: ScanLimits) -> Self { Self { limits, cancelled: Arc::new(AtomicBool::new(false)) } }
    // Existing administrative/native APIs retain their compatibility contract.
    // SQL query paths must always use default or explicitly supplied limits.
    pub(crate) fn legacy_unlimited() -> Self {
        Self::new(ScanLimits { max_rows: usize::MAX, max_bytes: usize::MAX, max_work: usize::MAX })
    }
    pub fn limits(&self) -> ScanLimits { self.limits }
    pub fn cancel(&self) { self.cancelled.store(true, Ordering::Relaxed); }
    pub fn check(&self) -> Result<(), ResourceError> {
        if self.cancelled.load(Ordering::Relaxed) { Err(ResourceError::Cancelled) } else { Ok(()) }
    }
    pub(crate) fn cap_rows(&self, rows: usize) -> Self {
        Self { limits: ScanLimits { max_rows: self.limits.max_rows.min(rows), ..self.limits }, cancelled: Arc::clone(&self.cancelled) }
    }
    pub(crate) fn meter(&self) -> Result<ScanMeter<'_>, ResourceError> {
        self.check()?;
        Ok(ScanMeter { control: self, rows: 0, bytes: 0, work: 0 })
    }
}

pub(crate) struct ScanMeter<'a> { control: &'a ScanControl, rows: usize, bytes: usize, work: usize }
impl ScanMeter<'_> {
    pub(crate) fn step(&mut self) -> Result<(), ResourceError> {
        self.control.check()?;
        self.work = self.work.checked_add(1).filter(|value| *value <= self.control.limits.max_work)
            .ok_or(ResourceError::Limit { resource: "work", limit: self.control.limits.max_work })?;
        Ok(())
    }
    pub(crate) fn row(&mut self, bytes: usize) -> Result<(), ResourceError> {
        self.control.check()?;
        self.rows = self.rows.checked_add(1).filter(|value| *value <= self.control.limits.max_rows)
            .ok_or(ResourceError::Limit { resource: "rows", limit: self.control.limits.max_rows })?;
        self.bytes = self.bytes.checked_add(bytes).filter(|value| *value <= self.control.limits.max_bytes)
            .ok_or(ResourceError::Limit { resource: "bytes", limit: self.control.limits.max_bytes })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_is_shared_and_checked_after_work_has_started() {
        let control = ScanControl::default();
        let capped = control.cap_rows(1);
        let mut meter = capped.meter().unwrap();
        meter.step().unwrap(); meter.row(1).unwrap();
        std::thread::spawn(move || control.cancel()).join().unwrap();
        assert_eq!(meter.step(), Err(ResourceError::Cancelled));
        assert_eq!(meter.row(1), Err(ResourceError::Cancelled));
    }
    #[test]
    fn accounting_overflow_never_wraps_or_disables_limits() {
        let control = ScanControl::new(ScanLimits { max_rows: usize::MAX, max_bytes: usize::MAX, max_work: usize::MAX });
        let mut meter = control.meter().unwrap();
        meter.row(usize::MAX).unwrap();
        assert!(matches!(meter.row(1), Err(ResourceError::Limit { resource: "bytes", .. })));
        meter.work = usize::MAX;
        assert!(matches!(meter.step(), Err(ResourceError::Limit { resource: "work", .. })));
    }
}

/// Per-domain retained-state estimate; not a total process RSS reservation.
#[derive(Clone, Copy, Debug)]
pub struct HistoryLimits {
    pub max_keys: usize,
    pub max_versions: usize,
    pub max_bytes: usize,
}
impl Default for HistoryLimits {
    fn default() -> Self { Self { max_keys: 1000000, max_versions: 1000000, max_bytes: 256 * 1024 * 1024 } }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HistoryUsage { pub keys: usize, pub versions: usize, pub bytes: usize }
impl HistoryLimits {
    pub(crate) fn check(&self, usage: HistoryUsage) -> Result<(), ResourceError> {
        for (resource, actual, limit) in [("keys",usage.keys,self.max_keys),("versions",usage.versions,self.max_versions),("bytes",usage.bytes,self.max_bytes)] {
            if actual > limit { return Err(ResourceError::HistoryLimit { resource,limit }); }
        }
        Ok(())
    }
}

/// Accounted pending mutations and savepoint copies in one historical transaction.
/// The physical WAL ceiling remains 256 distinct staged keys even with a higher policy.
#[derive(Clone, Copy, Debug)]
pub struct StagingLimits { pub max_keys: usize, pub max_bytes: usize }
impl Default for StagingLimits {
    fn default() -> Self { Self { max_keys: 256, max_bytes: 8 * 1024 * 1024 } }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StagingUsage { pub keys: usize, pub bytes: usize }
impl StagingLimits {
    pub(crate) fn check(&self, usage: StagingUsage) -> Result<(), ResourceError> {
        for (resource, actual, limit) in [("keys",usage.keys,self.max_keys.min(256)),("bytes",usage.bytes,self.max_bytes)] {
            if actual > limit { return Err(ResourceError::StagingLimit { resource,limit }); }
        }
        Ok(())
    }
}

/// Separate live-row and WAL-delta map budgets; excludes allocator slack and I/O buffers.
#[derive(Clone, Copy, Debug)]
pub struct RowStoreLimits {
    pub max_rows: usize,
    pub max_row_bytes: usize,
    pub max_delta_keys: usize,
    pub max_delta_bytes: usize,
}
impl Default for RowStoreLimits {
    fn default() -> Self { Self { max_rows: 2000000, max_row_bytes: 256 * 1024 * 1024,
        max_delta_keys: 2000000, max_delta_bytes: 256 * 1024 * 1024 } }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowStoreUsage { pub rows: usize, pub row_bytes: usize, pub delta_keys: usize, pub delta_bytes: usize }
impl RowStoreLimits {
    pub(crate) fn check(&self, usage: RowStoreUsage) -> Result<(), ResourceError> {
        for (resource, actual, limit) in [("rows",usage.rows,self.max_rows),("row bytes",usage.row_bytes,self.max_row_bytes),
            ("delta keys",usage.delta_keys,self.max_delta_keys),("delta bytes",usage.delta_bytes,self.max_delta_bytes)] {
            if actual > limit { return Err(ResourceError::RowStoreLimit { resource,limit }); }
        }
        Ok(())
    }
}

/// Retained checkpoint key/pointer entries used by derived-index audit/rebuild.
#[derive(Clone, Copy, Debug)]
pub struct IndexLimits { pub max_entries: usize, pub max_bytes: usize }
impl Default for IndexLimits {
    fn default() -> Self { Self { max_entries: 2000000, max_bytes: 256 * 1024 * 1024 } }
}

/// Conservative checkpoint pointer-vector plus index descriptor reservation.
#[derive(Clone, Copy, Debug)]
pub struct IndexBuildLimits { pub max_entries: usize, pub max_bytes: usize }
impl Default for IndexBuildLimits {
    fn default() -> Self { Self { max_entries: 2000000, max_bytes: 512 * 1024 * 1024 } }
}
/// Process-local native storage policies; none changes persisted bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct StorageLimits { pub rows: RowStoreLimits, pub index: IndexLimits, pub build: IndexBuildLimits }
