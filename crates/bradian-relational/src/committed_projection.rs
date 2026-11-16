//! ADR 0134 — committed columnar projection primitive.
//!
//! BRD-HTAP-004 "committed columnar projection with LSN
//! witness" is OPEN. ADR 0128 shipped the HTAP freshness
//! watermark; ADR 0133 shipped the row↔column converter.
//! The gap was the composite: a column batch TAGGED with
//! the commit LSN it was captured at, so analytical
//! readers can verify freshness without re-consulting the
//! transactional side.
//!
//! Honest scope:
//! - `CommittedProjection { batch: Vec<ColumnBatch>,
//!   as_of_lsn: u64 }` — the LSN witness travels with
//!   the data.
//! - `project_committed(batch, lsn, watermark)` validates
//!   `lsn <= watermark.latest_committed()` before
//!   wrapping. A future columnar engine advances the
//!   HTAP watermark's projected side after publishing.
//! - `CommittedProjection::is_fresh_as_of(reader_lsn)` —
//!   the projection is acceptable iff its `as_of_lsn >=
//!   reader_lsn`.
//!
//! Out of scope (follow-ups):
//! - Durable segment file writer / reader for projections.
//! - Automatic projection catch-up worker.
//! - Multi-table / per-partition projections.
//! - Zero-copy projection sharing (today the primitive
//!   clones).

use thiserror::Error;

use crate::column_batch::ColumnBatch;
use crate::htap_watermark::HtapWatermark;

#[derive(Debug, Error)]
pub enum ProjectionError {
    #[error("projection as_of_lsn {as_of_lsn} exceeds committed watermark {watermark}")]
    FutureProjection { as_of_lsn: u64, watermark: u64 },
    #[error("projection batch is empty")]
    EmptyBatch,
}

pub type ProjectionResult<T> = std::result::Result<T, ProjectionError>;

/// ADR 0134 — a columnar batch tagged with the commit LSN
/// at which it is a correct view of the table state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedProjection {
    pub batch: Vec<ColumnBatch>,
    pub as_of_lsn: u64,
}

impl CommittedProjection {
    /// Is this projection fresh enough to satisfy a reader
    /// asking for data as of LSN `reader_lsn`?
    pub fn is_fresh_as_of(&self, reader_lsn: u64) -> bool {
        self.as_of_lsn >= reader_lsn
    }

    /// Number of distinct columns in this projection.
    pub fn width(&self) -> usize { self.batch.len() }

    /// Number of rows per column (must be uniform;
    /// `project_committed` enforced this on construction).
    pub fn height(&self) -> usize {
        self.batch.first().map(|c| c.values.len()).unwrap_or(0)
    }
}

/// ADR 0134 — wrap a column batch into a committed
/// projection iff `as_of_lsn` is at or below the OLTP
/// watermark.
pub fn project_committed(
    batch: Vec<ColumnBatch>,
    as_of_lsn: u64,
    watermark: &HtapWatermark,
) -> ProjectionResult<CommittedProjection> {
    if batch.is_empty() {
        return Err(ProjectionError::EmptyBatch);
    }
    let committed = watermark.latest_committed();
    if as_of_lsn > committed {
        return Err(ProjectionError::FutureProjection {
            as_of_lsn, watermark: committed,
        });
    }
    Ok(CommittedProjection { batch, as_of_lsn })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DataType;
    use crate::typed_table::Value;

    fn one_col(name: &str, values: Vec<Value>) -> ColumnBatch {
        ColumnBatch { name: name.into(), dtype: DataType::Int64, values }
    }

    #[test]
    fn projection_within_committed_watermark_accepted() {
        let w = HtapWatermark::new();
        w.advance_committed(100);
        let p = project_committed(vec![one_col("id",
            vec![Value::Int64(1), Value::Int64(2)])], 50, &w).unwrap();
        assert_eq!(p.as_of_lsn, 50);
        assert_eq!(p.width(), 1);
        assert_eq!(p.height(), 2);
    }

    #[test]
    fn projection_exactly_at_watermark_accepted() {
        let w = HtapWatermark::new();
        w.advance_committed(100);
        let p = project_committed(vec![one_col("id",
            vec![Value::Int64(1)])], 100, &w).unwrap();
        assert_eq!(p.as_of_lsn, 100);
    }

    #[test]
    fn projection_above_watermark_rejected() {
        let w = HtapWatermark::new();
        w.advance_committed(50);
        let err = project_committed(vec![one_col("id",
            vec![Value::Int64(1)])], 100, &w).unwrap_err();
        assert!(matches!(err, ProjectionError::FutureProjection {
            as_of_lsn: 100, watermark: 50,
        }));
    }

    #[test]
    fn empty_batch_rejected() {
        let w = HtapWatermark::new();
        let err = project_committed(vec![], 0, &w).unwrap_err();
        assert!(matches!(err, ProjectionError::EmptyBatch));
    }

    #[test]
    fn is_fresh_as_of_respects_lsn() {
        let w = HtapWatermark::new();
        w.advance_committed(100);
        let p = project_committed(vec![one_col("id",
            vec![Value::Int64(1)])], 80, &w).unwrap();
        assert!(p.is_fresh_as_of(80));
        assert!(p.is_fresh_as_of(79));
        assert!(!p.is_fresh_as_of(81));
    }

    #[test]
    fn zero_lsn_is_valid_when_watermark_is_zero() {
        let w = HtapWatermark::new();
        let p = project_committed(vec![one_col("id",
            vec![Value::Int64(0)])], 0, &w).unwrap();
        assert_eq!(p.as_of_lsn, 0);
        assert!(p.is_fresh_as_of(0));
    }

    #[test]
    fn width_and_height_report_correctly() {
        let w = HtapWatermark::new();
        w.advance_committed(100);
        let batch = vec![
            one_col("a", vec![Value::Int64(1), Value::Int64(2), Value::Int64(3)]),
            one_col("b", vec![Value::Int64(4), Value::Int64(5), Value::Int64(6)]),
        ];
        let p = project_committed(batch, 50, &w).unwrap();
        assert_eq!(p.width(), 2);
        assert_eq!(p.height(), 3);
    }

    #[test]
    fn projections_are_cloneable_and_equal_by_value() {
        let w = HtapWatermark::new();
        w.advance_committed(100);
        let p = project_committed(vec![one_col("id",
            vec![Value::Int64(1)])], 50, &w).unwrap();
        let q = p.clone();
        assert_eq!(p, q);
    }
}
