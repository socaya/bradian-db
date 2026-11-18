//! ADR 0164 — projector scheduler.
//!
//! Composes:
//! - ADR 0128 `HtapWatermark` — the shared freshness state.
//! - ADR 0159 `projector_worker::drain_once` — the inner
//!   per-batch step.
//! - ADR 0163 `projection_segment::write_segment` —
//!   durable sink.
//!
//! The scheduler owns a source iterator factory (caller
//! supplies, to allow re-pulling batches between
//! iterations) + a target directory where segments are
//! written. `run_bounded(max_iterations)` loops until the
//! source dries up, the iteration cap is hit, or the
//! cancellation handle fires.
//!
//! Honest scope — synchronous, single-threaded, bounded.
//! No wall-clock interval (caller inserts `sleep`
//! between invocations if needed). This is the honest
//! foundation; tokio-driven async scheduling is a
//! follow-up.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use thiserror::Error;

use crate::htap_watermark::HtapWatermark;
use crate::projection_segment::{write_segment, SegmentError};
use crate::projector_worker::{drain_once, ProjectorError, SourceBatch};

#[derive(Debug, Error)]
pub enum SchedulerError {
    #[error(transparent)]
    Projector(#[from] ProjectorError),
    #[error(transparent)]
    Segment(#[from] SegmentError),
    #[error("scheduler cancelled")]
    Cancelled,
}

pub type SchedulerResult<T> = std::result::Result<T, SchedulerError>;

/// ADR 0164 — scheduler run report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScheduleReport {
    pub iterations_run: u32,
    pub projections_written: u64,
    pub last_segment: Option<PathBuf>,
}

/// ADR 0164 — synchronous scheduler.
pub struct ProjectorScheduler {
    watermark: HtapWatermark,
    target_dir: PathBuf,
    cancellation: Arc<AtomicBool>,
}

impl ProjectorScheduler {
    pub fn new(watermark: HtapWatermark, target_dir: impl Into<PathBuf>) -> Self {
        Self {
            watermark, target_dir: target_dir.into(),
            cancellation: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Shared cancellation handle. Flip from another
    /// thread to stop the scheduler at the next iteration
    /// boundary.
    pub fn cancellation_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancellation)
    }

    pub fn watermark(&self) -> &HtapWatermark { &self.watermark }
    pub fn target_dir(&self) -> &Path { &self.target_dir }

    /// ADR 0164 — run up to `max_iterations` of the
    /// scheduler loop. Each iteration calls `source_fn`
    /// to obtain the next batch set, then drains it,
    /// writing segments for each successful projection.
    pub fn run_bounded<F, I>(
        &self,
        max_iterations: u32,
        mut source_fn: F,
    ) -> SchedulerResult<ScheduleReport>
    where
        F: FnMut() -> I,
        I: IntoIterator<Item = SourceBatch>,
    {
        let mut report = ScheduleReport::default();
        for _ in 0..max_iterations {
            if self.cancellation.load(Ordering::Relaxed) {
                return Err(SchedulerError::Cancelled);
            }
            let source = source_fn();
            let mut source_vec: Vec<SourceBatch> = source.into_iter().collect();
            if source_vec.is_empty() {
                report.iterations_run += 1;
                break;
            }
            let dir = &self.target_dir;
            // Take source_vec by value and consume the batches
            // ONE AT A TIME so that if sink fails we have the
            // same guarantee as drain_once: prior progress is
            // preserved on the watermark.
            let mut last = None;
            let drained = drain_once(
                &self.watermark,
                source_vec.drain(..),
                |projection| {
                    let filename = format!("p-{:020}.brps", projection.as_of_lsn);
                    let path = dir.join(&filename);
                    write_segment(&path, &projection)
                        .map_err(|e| format!("{e}"))?;
                    last = Some(path);
                    Ok(())
                },
            )?;
            report.iterations_run += 1;
            report.projections_written += drained;
            if let Some(p) = last { report.last_segment = Some(p); }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::column_batch::RowBatch;
    use crate::model::DataType;
    use crate::projection_segment::read_segment;
    use crate::typed_table::Value;
    use tempfile::tempdir;

    fn batch(lsn: u64, ids: &[i64]) -> SourceBatch {
        SourceBatch {
            as_of_lsn: lsn,
            batch: RowBatch {
                columns: vec![("id".into(), DataType::Int64)],
                rows: ids.iter().map(|id| vec![Value::Int64(*id)]).collect(),
            },
        }
    }

    #[test]
    fn scheduler_writes_a_segment_per_committed_projection() {
        let dir = tempdir().unwrap();
        let w = HtapWatermark::new();
        w.advance_committed(100);
        let sched = ProjectorScheduler::new(w, dir.path());
        let mut round = 0;
        let report = sched.run_bounded(10, || {
            round += 1;
            match round {
                1 => vec![batch(10, &[1]), batch(20, &[2])],
                _ => vec![],  // exhausted
            }
        }).unwrap();
        assert_eq!(report.projections_written, 2);
        assert!(report.last_segment.is_some());
        // Segments round-trip.
        for lsn in [10u64, 20] {
            let path = dir.path().join(format!("p-{lsn:020}.brps"));
            let p = read_segment(&path).unwrap();
            assert_eq!(p.as_of_lsn, lsn);
        }
    }

    #[test]
    fn scheduler_stops_on_empty_source() {
        let dir = tempdir().unwrap();
        let w = HtapWatermark::new();
        w.advance_committed(100);
        let sched = ProjectorScheduler::new(w, dir.path());
        let report = sched.run_bounded(10, Vec::<SourceBatch>::new).unwrap();
        assert_eq!(report.iterations_run, 1);
        assert_eq!(report.projections_written, 0);
    }

    #[test]
    fn scheduler_cancellation_stops_at_next_iteration() {
        let dir = tempdir().unwrap();
        let w = HtapWatermark::new();
        w.advance_committed(1000);
        let sched = ProjectorScheduler::new(w, dir.path());
        let cancel = sched.cancellation_handle();
        let mut round = 0;
        let err = sched.run_bounded(10, || {
            round += 1;
            if round == 2 {
                // Fire cancellation before the next iteration.
                cancel.store(true, Ordering::Relaxed);
            }
            vec![batch(round, &[1])]
        }).unwrap_err();
        assert!(matches!(err, SchedulerError::Cancelled));
    }

    #[test]
    fn scheduler_advances_watermark_through_successful_batches() {
        let dir = tempdir().unwrap();
        let w = HtapWatermark::new();
        w.advance_committed(100);
        let sched = ProjectorScheduler::new(w, dir.path());
        let mut round = 0;
        sched.run_bounded(5, || {
            round += 1;
            if round == 1 { vec![batch(42, &[1])] } else { vec![] }
        }).unwrap();
        assert_eq!(sched.watermark().latest_projected(), 42);
    }

    #[test]
    fn report_counts_iterations_and_written() {
        let dir = tempdir().unwrap();
        let w = HtapWatermark::new();
        w.advance_committed(100);
        let sched = ProjectorScheduler::new(w, dir.path());
        let mut round = 0;
        let report = sched.run_bounded(5, || {
            round += 1;
            match round {
                1 => vec![batch(5, &[1]), batch(6, &[1])],
                2 => vec![batch(7, &[1])],
                _ => vec![],
            }
        }).unwrap();
        assert_eq!(report.projections_written, 3);
        assert!(report.iterations_run >= 2);
    }
}
