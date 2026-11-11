//! Vectorized query runtime.
//!
//! Implements RUN-001..004 from the traceability register:
//! - Typed `RecordBatch` with validity bitmaps and selection-vector-friendly ownership.
//! - Scan over `.derc` segments and in-memory delta (via the storage `Partition`).
//! - Filter, project, hash aggregate, sort/top-k.
//! - A scalar reference "oracle" that re-executes the same logical plan row-by-row for
//!   property tests.
#![deny(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod aggregate;
pub mod batch;
pub mod budget;
pub mod expr;
pub mod oracle;
pub mod scan;
pub mod sort;

pub use aggregate::{AggregateKind, HashAggregator};
pub use batch::{RecordBatch, RecordBatchBuilder};
pub use budget::{ExecContext, MemoryBudget, RuntimeMetrics};
pub use expr::{Expr, Predicate};
pub use scan::{scan_partition, scan_segments, ScanOptions};
pub use sort::{sort_batch, SortSpec, TopK};
