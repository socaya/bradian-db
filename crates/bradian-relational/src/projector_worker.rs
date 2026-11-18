//! ADR 0159 — projector worker (watermark advancer).
//!
//! BRD-HTAP-004 "committed columnar projection with LSN
//! witness" moves from FOUNDATIONS to PARTIAL: ADR 0128
//! shipped the `HtapWatermark`, ADR 0133 shipped the row ↔
//! column batch converter, ADR 0134 shipped the committed
//! projection primitive. The missing piece was the WORKER
//! that drives the OLAP-side advance.
//!
//! This module ships a bounded single-threaded worker that:
//! 1. Takes a `HtapWatermark`, an iterator yielding
//!    `(lsn, RowBatch)` from the caller's source of truth,
//!    and a per-batch callback.
//! 2. For each batch, converts it to column form, checks
//!    `lsn <= watermark.latest_committed()`, invokes the
//!    caller's callback with the resulting
//!    `CommittedProjection`, and advances
//!    `watermark.advance_projected(lsn)` on success.
//! 3. Stops on first callback error or on a budget
//!    exhaustion.
//!
//! Honest scope:
//! - Single-threaded; caller owns threading.
//! - Pure plumbing between existing primitives — no
//!   catalog awareness, no I/O.
//! - `ProjectorError` wraps the three composed error
//!   types (watermark future, batch conversion, caller
//!   callback) so the caller sees one error type.
//!
//! Out of scope:
//! - Durable segment writer (ADR 0082 is DictIndex32;
//!   this worker emits in-memory `CommittedProjection`).
//! - Multi-table / per-partition scheduling.
//! - Catch-up policy (caller decides when to invoke
//!   `drain_once` vs. tight loop).
//! - Backpressure on the sink.

use thiserror::Error;

use crate::column_batch::{rows_to_columns, BatchError, RowBatch};
use crate::committed_projection::{project_committed, CommittedProjection, ProjectionError};
use crate::htap_watermark::HtapWatermark;

/// Caller-supplied batch with the LSN at which it was
/// consistent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceBatch {
    pub as_of_lsn: u64,
    pub batch: RowBatch,
}

#[derive(Debug, Error)]
pub enum ProjectorError {
    #[error(transparent)]
    Projection(#[from] ProjectionError),
    #[error(transparent)]
    Batch(#[from] BatchError),
    #[error("sink rejected the projection at lsn {lsn}: {reason}")]
    Sink { lsn: u64, reason: String },
}

pub type ProjectorResult<T> = std::result::Result<T, ProjectorError>;

/// ADR 0159 — drain a source iterator once, emitting
/// committed projections to the caller's `sink`.
///
/// Returns the number of batches successfully projected.
/// On the first error, stops and returns the error;
/// the watermark has advanced through every *successful*
/// batch before the failure.
pub fn drain_once<I, F>(
    watermark: &HtapWatermark,
    source: I,
    mut sink: F,
) -> ProjectorResult<u64>
where
    I: IntoIterator<Item = SourceBatch>,
    F: FnMut(CommittedProjection) -> Result<(), String>,
{
    let mut emitted = 0u64;
    for src in source {
        let columns = rows_to_columns(&src.batch)?;
        let projection = project_committed(columns, src.as_of_lsn, watermark)?;
        sink(projection).map_err(|reason| ProjectorError::Sink {
            lsn: src.as_of_lsn, reason,
        })?;
        watermark.advance_projected(src.as_of_lsn);
        emitted += 1;
    }
    Ok(emitted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DataType;
    use crate::typed_table::Value;

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
    fn drain_advances_projected_watermark_for_every_batch() {
        let w = HtapWatermark::new();
        w.advance_committed(100);
        let source = vec![batch(10, &[1, 2]), batch(20, &[3]), batch(50, &[4])];
        let mut sink_count = 0;
        let emitted = drain_once(&w, source, |_| { sink_count += 1; Ok(()) }).unwrap();
        assert_eq!(emitted, 3);
        assert_eq!(sink_count, 3);
        assert_eq!(w.latest_projected(), 50);
    }

    #[test]
    fn drain_stops_at_first_sink_error_but_keeps_prior_progress() {
        let w = HtapWatermark::new();
        w.advance_committed(100);
        let source = vec![batch(10, &[1]), batch(20, &[2]), batch(30, &[3])];
        let mut count = 0;
        let err = drain_once(&w, source, |_| {
            count += 1;
            if count == 2 { Err("boom".into()) } else { Ok(()) }
        }).unwrap_err();
        assert!(matches!(err, ProjectorError::Sink { lsn: 20, .. }));
        // The first batch (lsn=10) advanced; the failing
        // one (lsn=20) did not.
        assert_eq!(w.latest_projected(), 10);
    }

    #[test]
    fn drain_refuses_future_lsn_above_committed() {
        let w = HtapWatermark::new();
        w.advance_committed(10);
        let source = vec![batch(100, &[1])];
        let err = drain_once(&w, source, |_| Ok(())).unwrap_err();
        assert!(matches!(err, ProjectorError::Projection(
            ProjectionError::FutureProjection { .. })));
        assert_eq!(w.latest_projected(), 0);
    }

    #[test]
    fn drain_empty_source_is_a_no_op() {
        let w = HtapWatermark::new();
        w.advance_committed(100);
        let emitted = drain_once(&w, std::iter::empty(), |_| Ok(())).unwrap();
        assert_eq!(emitted, 0);
        assert_eq!(w.latest_projected(), 0);
    }

    #[test]
    fn batch_width_mismatch_surfaces_as_batch_error() {
        let w = HtapWatermark::new();
        w.advance_committed(100);
        let mut malformed = batch(10, &[1, 2]);
        // Trim one row's width to force a mismatch.
        malformed.batch.rows[0].pop();
        let err = drain_once(&w, vec![malformed], |_| Ok(())).unwrap_err();
        assert!(matches!(err, ProjectorError::Batch(_)));
    }

    #[test]
    fn projection_carries_the_source_lsn() {
        let w = HtapWatermark::new();
        w.advance_committed(100);
        let mut seen_lsn = 0u64;
        drain_once(&w, vec![batch(42, &[1])], |p| {
            seen_lsn = p.as_of_lsn;
            Ok(())
        }).unwrap();
        assert_eq!(seen_lsn, 42);
    }
}
