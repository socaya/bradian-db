//! Query-buffer/work and retained-history admission limits; not a total RSS reservation.
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
    #[error("retained history {resource} limit exceeded (limit {limit})")]
    HistoryLimit { resource: &'static str, limit: usize },
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
