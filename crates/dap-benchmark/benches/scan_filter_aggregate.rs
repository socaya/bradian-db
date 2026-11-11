//! Placeholder bench — real benches land with `dap-runtime` operators (task #13).
use criterion::{criterion_group, criterion_main, Criterion};

fn noop(c: &mut Criterion) {
    c.bench_function("noop", |b| b.iter(|| 1 + 1));
}

criterion_group!(benches, noop);
criterion_main!(benches);
