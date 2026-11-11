//! Real benches over the vectorized runtime using synthetic DHIS2 facts.

use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use dap_benchmark::generate_facts;
use dap_core::Value;
use dap_runtime::aggregate::{AggregateKind, HashAggregator};
use dap_runtime::batch::RecordBatchBuilder;
use dap_runtime::budget::ExecContext;
use dap_runtime::expr::{CmpOp, Expr, Predicate};
use dap_runtime::scan::{scan_partition, ScanOptions};
use dap_storage::{Partition, PartitionOptions};
use tempfile::TempDir;

struct Bench {
    _dir: TempDir,
    partition: Arc<Partition>,
}

fn setup(rows: usize) -> Bench {
    let dir = TempDir::new().unwrap();
    let fixture = generate_facts(42, 50, 24, 20, rows);
    let p = Partition::open(
        dir.path(),
        PartitionOptions {
            dataset_id: "bench".into(),
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
    Bench {
        _dir: dir,
        partition: p,
    }
}

fn scan_only(c: &mut Criterion) {
    let bench = setup(10_000);
    let mut group = c.benchmark_group("scan");
    group.throughput(Throughput::Elements(10_000));
    group.bench_function("scan_10k", |b| {
        b.iter(|| {
            let ctx = ExecContext::default();
            let batches =
                scan_partition(&bench.partition, &ScanOptions::default(), &ctx).unwrap();
            black_box(batches.len());
        });
    });
    group.finish();
}

fn filter_aggregate(c: &mut Criterion) {
    let bench = setup(10_000);
    let mut group = c.benchmark_group("filter_aggregate");
    group.throughput(Throughput::Elements(10_000));
    group.bench_function("sum_by_ou_10k", |b| {
        b.iter(|| {
            let ctx = ExecContext::default();
            let batches =
                scan_partition(&bench.partition, &ScanOptions::default(), &ctx).unwrap();
            let pred = Predicate::Compare {
                left: Expr::Col("value".into()),
                op: CmpOp::GtEq,
                right: Expr::Lit(Value::F64(0.0)),
            };
            let mut agg = HashAggregator::new(
                bench.partition.schema().clone(),
                vec!["ou".into()],
                vec![AggregateKind::Sum { column: "value".into() }],
            )
            .unwrap();
            for batch in &batches {
                let sel = pred.eval_batch(batch).unwrap();
                let mut b = RecordBatchBuilder::new(batch.schema.clone());
                for r in sel {
                    b.append_row(&batch.row_values(r as usize).unwrap()).unwrap();
                }
                agg.consume_batch(&b.finish().unwrap(), &ctx).unwrap();
            }
            let out = agg.finish(&ctx).unwrap();
            black_box(out.row_count());
        });
    });
    group.finish();
}

criterion_group!(benches, scan_only, filter_aggregate);
criterion_main!(benches);
