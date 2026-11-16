//! ADR 0108 — BACE sweep benchmark corpus.
//!
//! Sweeps `max_rows_per_page` across {8, 64, 512, 4096} on
//! the ADR 0094 DHIS2-like synthetic workload. Produces a
//! size + time table operators can consult to pick a page
//! size for their own schemas.
//!
//! Honest: still synthetic. Real DHIS2 production data is
//! out of scope (owner procurement).

use std::sync::Arc;
use std::time::Instant;

use bradian_benchmark::fixtures::generate_facts;
use bradian_relational::bace::codec::raw::RAW_ID;
use bradian_relational::bace::codec::zstd::ZSTD_ID;
use bradian_relational::bace::policy::{
    CompressionProfile, InMemoryPolicyService, PlatformBaseline,
};
use bradian_relational::bace::registry::BuiltInRegistry;
use bradian_relational::row_store::BaceWiring;
use bradian_storage::{SegmentReader, SegmentWriter, SegmentWriterOptions};
use criterion::{criterion_group, criterion_main, Criterion};
use tempfile::tempdir;

fn wiring() -> BaceWiring {
    let baseline = PlatformBaseline {
        profile: CompressionProfile::Balanced,
        allowed_codecs: vec![ZSTD_ID, RAW_ID],
        ..PlatformBaseline::shipped()
    };
    BaceWiring {
        policy_service: Arc::new(InMemoryPolicyService::new(baseline, BuiltInRegistry::new())),
        registry: Arc::new(BuiltInRegistry::new()),
        authentication: None,
        strict_authenticated: false,
        previous_authentication: None,
    }
}

fn opts(max_rows_per_page: usize, use_column_dict: bool) -> SegmentWriterOptions {
    SegmentWriterOptions {
        dataset_id: "dhis2".into(),
        partition_id: "p".into(),
        segment_id: "s".into(),
        compress: true,
        dict_cardinality_threshold: 0.5,
        watermark_ms: 0,
        data_revision: 0,
        metadata_revision: None,
        max_rows_per_page: Some(max_rows_per_page),
        use_column_dict,
    }
}

fn sweep(c: &mut Criterion) {
    // 36k rows, 5 columns, same shape as dhis2_like.
    let fx = generate_facts(42, 50, 12, 20, 36_000);
    println!("[ADR0108 sweep] {} rows × {} cols", fx.rows.len(), fx.schema.len());

    let dir = tempdir().unwrap();
    let variants: &[(&str, usize, bool)] = &[
        ("page=8   plain",   8,    false),
        ("page=64  plain",   64,   false),
        ("page=512 plain",   512,  false),
        ("page=4096 plain",  4096, false),
        ("page=8   coldict",   8,    true),
        ("page=64  coldict",   64,   true),
        ("page=512 coldict",  512,  true),
        ("page=4096 coldict", 4096, true),
    ];
    for (label, mrpp, cd) in variants {
        let safe: String = label.chars()
            .map(|c| if c == ' ' || c == '=' { '_' } else { c }).collect();
        let path = dir.path().join(format!("{safe}.brcs"));
        let start = Instant::now();
        let mut w = SegmentWriter::new_with_bace(
            fx.schema.clone(), opts(*mrpp, *cd), wiring(),
        );
        for row in &fx.rows { w.append_row(row).unwrap(); }
        w.finish_to_path(&path).unwrap();
        let write_ms = start.elapsed().as_secs_f64() * 1000.0;
        let bytes = std::fs::metadata(&path).unwrap().len();

        let rstart = Instant::now();
        let mut r = SegmentReader::open_with_bace(&path, wiring()).unwrap();
        let mut rows = 0usize;
        for col in r.columns().iter().map(|c| c.name.clone()).collect::<Vec<_>>() {
            rows += r.read_column(&col).unwrap().len();
        }
        let read_ms = rstart.elapsed().as_secs_f64() * 1000.0;
        println!(
            "[ADR0108] {label:22} bytes={bytes:>10} write={write_ms:>6.1}ms read={read_ms:>6.1}ms rows={rows}",
        );
    }

    // Single-sample criterion function so the bench harness
    // has something to call.
    c.bench_function("adr0108_sweep", |b| b.iter(|| {}));
}

criterion_group!(benches, sweep);
criterion_main!(benches);
