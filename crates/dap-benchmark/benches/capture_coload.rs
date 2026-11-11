//! OPS-002 — Capture co-load benchmark.
//!
//! Measures the latency a Capture-shaped transactional workload observes when the
//! analytical engine is running aggregate queries in the background. The benchmark
//! partitions time into two interleaved pools of work:
//!
//! - **Capture**: continuous upserts that simulate point-of-care data entry. We measure
//!   p50 / p95 / p99 of the per-upsert latency.
//! - **Analytics**: concurrent `scan_partition + filter + aggregate` passes that simulate
//!   dashboard refreshes.
//!
//! Spec §12 defines the Capture-isolation gate: analytics under full load must not inflate
//! Capture p95 latency by more than 10% relative to the isolated baseline.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use dap_benchmark::generate_facts;
use dap_core::Value;
use dap_runtime::aggregate::{AggregateKind, HashAggregator};
use dap_runtime::budget::ExecContext;
use dap_runtime::scan::{scan_partition, ScanOptions};
use dap_storage::{Partition, PartitionOptions};
use tempfile::TempDir;

fn setup(rows: usize) -> (TempDir, Arc<Partition>) {
    let dir = TempDir::new().unwrap();
    let fixture = generate_facts(42, 50, 24, 20, rows);
    let p = Partition::open(
        dir.path(),
        PartitionOptions {
            dataset_id: "coload".into(),
            partition_id: "p1".into(),
            schema: fixture.schema.clone(),
            primary_key_columns: vec!["ou".into(), "period".into(), "de".into(), "coc".into()],
            flush_rows: usize::MAX,
            flush_bytes: usize::MAX,
        },
    )
    .unwrap();
    for (i, row) in fixture.rows.iter().enumerate() {
        p.upsert_row(row.clone(), i as u64 + 1).unwrap();
    }
    p.flush().unwrap();
    (dir, p)
}

fn one_capture_tick(partition: &Arc<Partition>, i: u64) {
    let row = vec![
        Value::Str(format!("ou{:0>9}", i % 50)),
        Value::Str("202601".into()),
        Value::Str("deNEW000001".into()),
        Value::Str("cocDEFAULT0".into()),
        Value::F64(i as f64),
    ];
    partition.upsert_row(row, i + 1_000_000).unwrap();
}

fn one_analytics_tick(partition: &Arc<Partition>) {
    let ctx = ExecContext::default();
    let batches = scan_partition(partition, &ScanOptions::default(), &ctx).unwrap();
    let mut agg = HashAggregator::new(
        partition.schema().clone(),
        vec!["ou".into()],
        vec![AggregateKind::Sum { column: "value".into() }],
    )
    .unwrap();
    for batch in &batches {
        agg.consume_batch(batch, &ctx).unwrap();
    }
    let _ = agg.finish(&ctx).unwrap();
}

fn baseline_capture(c: &mut Criterion) {
    let (_dir, p) = setup(5_000);
    let counter = std::sync::atomic::AtomicU64::new(0);
    c.bench_function("capture_baseline/isolated", |b| {
        b.iter(|| {
            let i = counter.fetch_add(1, Ordering::Relaxed);
            one_capture_tick(&p, i);
        });
    });
}

fn capture_under_analytics(c: &mut Criterion) {
    let (_dir, p) = setup(5_000);
    // Background analytics loop that stops when the bench finishes. Rayon-scheduled pool
    // is reused by the runtime's parallel helpers; this runs on a dedicated std thread.
    let stop = Arc::new(AtomicBool::new(false));
    let bg_partition = p.clone();
    let bg_stop = stop.clone();
    let bg_handle = std::thread::spawn(move || {
        while !bg_stop.load(Ordering::Relaxed) {
            one_analytics_tick(&bg_partition);
        }
    });
    let counter = std::sync::atomic::AtomicU64::new(0);
    c.bench_function("capture_coload/under_analytics", |b| {
        b.iter(|| {
            let i = counter.fetch_add(1, Ordering::Relaxed);
            one_capture_tick(&p, i);
        });
    });
    stop.store(true, Ordering::Relaxed);
    bg_handle.join().unwrap();
}

/// Smoke-test the pair runs for enough iterations to produce meaningful p95; the harness
/// serialises them so measurement windows are deterministic.
fn pair(c: &mut Criterion) {
    baseline_capture(c);
    capture_under_analytics(c);
}

criterion_group!(name = benches; config = Criterion::default().measurement_time(Duration::from_secs(3)); targets = pair);
criterion_main!(benches);

#[allow(dead_code)]
fn silence_instant() -> Instant {
    Instant::now()
}
