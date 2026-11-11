//! Scan operator: materialise rows from `.derc` segments + the in-memory delta of a Partition
//! into a sequence of `RecordBatch` values.
//!
//! Reads are snapshot-consistent: the scan holds a `Snapshot` handle for its lifetime and
//! reads only segments visible at that generation plus delta entries at the matching
//! delta-version.
//!
//! v1 scope: single-threaded, reads full segments into memory, then yields them chopped by
//! `ExecContext::batch_rows`. Later gates will add per-page iterators + prefetch.

use std::sync::Arc;

use dap_core::{DapResult, Schema};
use dap_storage::{ColumnBuffer, Partition, SegmentReader};

#[allow(unused_imports)]
use dap_storage::ManifestSegment as _ManifestSegment;

use crate::batch::{RecordBatch, RecordBatchBuilder};
use crate::budget::ExecContext;

#[derive(Clone, Debug, Default)]
pub struct ScanOptions {
    /// Project only these columns (by name). Empty = all columns.
    pub projection: Vec<String>,
}

/// Scan a partition's visible segments + in-memory delta, returning materialised batches.
///
/// Snapshot-consistent: reads the manifest + delta atomically via `Partition::snapshot_rows`
/// so compaction and ingestion cannot interleave inconsistent state into the output.
pub fn scan_partition(
    partition: &Arc<Partition>,
    options: &ScanOptions,
    ctx: &ExecContext,
) -> DapResult<Vec<RecordBatch>> {
    let _snapshot = partition.snapshot(); // retain for the duration of the scan
    let schema = partition.schema();
    let projected = project_schema(schema, options)?;

    let rows = partition.snapshot_rows()?;
    ctx.metrics.record_scan(rows.len() as u64, 0);

    // Project rows to the requested schema, slice into batches.
    let mut out = Vec::new();
    if rows.is_empty() {
        return Ok(out);
    }

    let projection_idx: Vec<usize> = projected
        .columns
        .iter()
        .map(|c| schema.index_of(&c.name).unwrap())
        .collect();

    let mut builder = RecordBatchBuilder::new(projected.clone());
    for row in &rows {
        ctx.check_cancellation()?;
        let projected_row: Vec<_> = projection_idx.iter().map(|&i| row.0[i].clone()).collect();
        builder.append_row(&projected_row)?;
        if builder.len() >= ctx.batch_rows {
            out.push(std::mem::replace(&mut builder, RecordBatchBuilder::new(projected.clone()))
                .finish()?);
        }
    }
    if !builder.is_empty() {
        out.push(builder.finish()?);
    }
    Ok(out)
}

/// Scan a single set of segment file paths. Used by the test harness without a full Partition.
pub fn scan_segments(
    paths: &[std::path::PathBuf],
    schema: &Schema,
    options: &ScanOptions,
    ctx: &ExecContext,
) -> DapResult<Vec<RecordBatch>> {
    let projected = project_schema(schema, options)?;
    let mut out = Vec::new();
    for p in paths {
        ctx.check_cancellation()?;
        let mut reader = SegmentReader::open(p)?;
        let n = reader.footer().row_count as usize;
        if n == 0 {
            continue;
        }
        let columns = read_columns(&mut reader, &projected)?;
        ctx.metrics.record_scan(n as u64, p.metadata().map(|m| m.len()).unwrap_or(0));
        let batch = RecordBatch::new(projected.clone(), columns)?;
        for sliced in slice_into_batches(batch, ctx.batch_rows)? {
            out.push(sliced);
        }
    }
    Ok(out)
}

fn project_schema(schema: &Schema, options: &ScanOptions) -> DapResult<Schema> {
    if options.projection.is_empty() {
        return Ok(schema.clone());
    }
    let mut cols = Vec::with_capacity(options.projection.len());
    for name in &options.projection {
        cols.push(schema.column(name)?.clone());
    }
    Schema::new(cols)
}

fn read_columns(
    reader: &mut SegmentReader,
    schema: &Schema,
) -> DapResult<Vec<ColumnBuffer>> {
    let mut out = Vec::with_capacity(schema.len());
    for c in &schema.columns {
        out.push(reader.read_column(&c.name)?);
    }
    Ok(out)
}

fn slice_into_batches(batch: RecordBatch, batch_rows: usize) -> DapResult<Vec<RecordBatch>> {
    if batch_rows == 0 || batch.row_count() <= batch_rows {
        return Ok(vec![batch]);
    }
    // v1 slicing: rebuild rows via RecordBatchBuilder. This is a correctness-first path; a
    // later gate will add zero-copy slicing with explicit (start, len) offsets.
    let mut out = Vec::new();
    let mut idx = 0usize;
    let total = batch.row_count();
    while idx < total {
        let end = (idx + batch_rows).min(total);
        let mut b = RecordBatchBuilder::new(batch.schema.clone());
        for r in idx..end {
            b.append_row(&batch.row_values(r)?)?;
        }
        out.push(b.finish()?);
        idx = end;
    }
    Ok(out)
}
