//! ADR 0094 — synthetic DHIS2-like fact-table benchmark.
//!
//! Compares `.brcs` size + read throughput across five
//! variants:
//! - v1.0 legacy (no BACE)
//! - v1.1 BACE
//! - v1.2 BACE + dict pool
//! - v1.4 column-level shared dict
//! - v1.4 + BACA (authenticated)
//!
//! Workload: deterministic synthetic DHIS2 facts generated
//! via the existing `FactsFixture`. Not real DHIS2 data.

use std::sync::Arc;
use std::time::Instant;

use bradian_benchmark::fixtures::generate_facts;
use bradian_relational::bace::authenticated::BaceAuthentication;
use bradian_relational::bace::codec::raw::RAW_ID;
use bradian_relational::bace::codec::zstd::ZSTD_ID;
use bradian_relational::bace::dictionary::train_zstd_dictionary;
use bradian_relational::bace::policy::{
    CompressionProfile, InMemoryPolicyService, PlatformBaseline,
};
use bradian_relational::bace::registry::BuiltInRegistry;
use bradian_relational::bace::BaceDictionary;
use bradian_relational::row_store::BaceWiring;
use bradian_storage::{SegmentReader, SegmentWriter, SegmentWriterOptions};
use criterion::{criterion_group, criterion_main, Criterion};
use tempfile::tempdir;

fn wiring(auth: Option<BaceAuthentication>) -> BaceWiring {
    let baseline = PlatformBaseline {
        profile: CompressionProfile::Balanced,
        allowed_codecs: vec![ZSTD_ID, RAW_ID],
        ..PlatformBaseline::shipped()
    };
    BaceWiring {
        policy_service: Arc::new(InMemoryPolicyService::new(baseline, BuiltInRegistry::new())),
        registry: Arc::new(BuiltInRegistry::new()),
        authentication: auth,
        strict_authenticated: false,
        previous_authentication: None,
    }
}

fn base_opts(column_dict: bool, split: Option<usize>) -> SegmentWriterOptions {
    SegmentWriterOptions {
        dataset_id: "dhis2".into(),
        partition_id: "p".into(),
        segment_id: "s".into(),
        compress: true,
        dict_cardinality_threshold: 0.5,
        watermark_ms: 0,
        data_revision: 0,
        metadata_revision: None,
        max_rows_per_page: split,
        use_column_dict: column_dict,
    }
}

fn write_variant(
    variant: &str,
    fixture: &bradian_benchmark::fixtures::FactsFixture,
    opts: SegmentWriterOptions,
    wiring_arg: Option<BaceWiring>,
    dicts: Option<std::collections::HashMap<String, BaceDictionary>>,
    path: &std::path::Path,
) -> (u64, std::time::Duration) {
    let mut w = match (wiring_arg, dicts) {
        (Some(w), Some(d)) => SegmentWriter::new_with_bace_and_dictionaries(
            fixture.schema.clone(), opts, w, d,
        ),
        (Some(w), None) => SegmentWriter::new_with_bace(
            fixture.schema.clone(), opts, w,
        ),
        (None, _) => SegmentWriter::new(fixture.schema.clone(), opts),
    };
    let start = Instant::now();
    for row in &fixture.rows {
        w.append_row(row).unwrap();
    }
    w.finish_to_path(path).unwrap();
    let elapsed = start.elapsed();
    let bytes = std::fs::metadata(path).unwrap().len();
    println!("[ADR0094 WRITE] {variant:20} {bytes:>10} bytes in {:>6.1} ms",
        elapsed.as_secs_f64() * 1000.0);
    (bytes, elapsed)
}

fn read_variant(
    variant: &str,
    path: &std::path::Path,
    wiring_arg: Option<BaceWiring>,
) {
    let start = Instant::now();
    let mut r = match wiring_arg {
        Some(w) => SegmentReader::open_with_bace(path, w).unwrap(),
        None => SegmentReader::open(path).unwrap(),
    };
    let mut total_rows = 0usize;
    for col in r.columns().iter().map(|c| c.name.clone()).collect::<Vec<_>>() {
        let buf = r.read_column(&col).unwrap();
        total_rows += buf.len();
    }
    let elapsed = start.elapsed();
    println!("[ADR0094 READ ] {variant:20} {total_rows:>10} rows in {:>6.1} ms",
        elapsed.as_secs_f64() * 1000.0);
}

fn dhis2_like_variants(c: &mut Criterion) {
    // 50 OUs × 12 periods × 20 DEs × 3 COCs = 36,000 rows.
    // Sized modestly so bench finishes quickly but still shows
    // meaningful BACE compression deltas.
    // (seed, org_units, periods, data_elements, rows)
    let fx = generate_facts(42, 50, 12, 20, 36_000);
    println!("[ADR0094] workload: {} rows, {} columns",
        fx.rows.len(), fx.schema.len());

    let dir = tempdir().unwrap();

    // v1.0 legacy — no BACE.
    let p_v10 = dir.path().join("v10.brcs");
    write_variant("v1.0 legacy", &fx, base_opts(false, None), None, None, &p_v10);

    // v1.1 BACE.
    let p_v11 = dir.path().join("v11.brcs");
    write_variant("v1.1 BACE", &fx, base_opts(false, None),
        Some(wiring(None)), None, &p_v11);

    // v1.2 BACE + dict pool: train on the first column's bytes.
    let samples: Vec<Vec<u8>> = fx.rows.iter().take(1000)
        .map(|r| match &r[0] {
            bradian_core::Value::Str(s) => s.as_bytes().to_vec(),
            bradian_core::Value::Uid(u) => u.as_ref().as_bytes().to_vec(),
            _ => Vec::new(),
        }).collect();
    let samples_ref: Vec<&[u8]> = samples.iter().map(|v| v.as_slice()).collect();
    let dict = train_zstd_dictionary(&samples_ref, 1024, "ou").unwrap();
    let mut dicts: std::collections::HashMap<String, BaceDictionary> =
        std::collections::HashMap::new();
    dicts.insert("ou".into(), dict);
    let p_v12 = dir.path().join("v12.brcs");
    write_variant("v1.2 BACE+dictpool", &fx, base_opts(false, None),
        Some(wiring(None)), Some(dicts), &p_v12);

    // v1.4 column-level shared dict (small pages so dict pays off).
    let p_v14 = dir.path().join("v14.brcs");
    write_variant("v1.4 column-dict", &fx, base_opts(true, Some(64)),
        Some(wiring(None)), None, &p_v14);

    // v1.4 + BACA.
    let key = [11u8; 32];
    let p_v14_baca = dir.path().join("v14_baca.brcs");
    write_variant("v1.4 + BACA", &fx, base_opts(true, Some(64)),
        Some(wiring(Some(BaceAuthentication::new(key, "bench")))), None, &p_v14_baca);

    // Read back each.
    read_variant("v1.0 legacy", &p_v10, None);
    read_variant("v1.1 BACE", &p_v11, Some(wiring(None)));
    read_variant("v1.2 BACE+dictpool", &p_v12, Some(wiring(None)));
    read_variant("v1.4 column-dict", &p_v14, Some(wiring(None)));
    read_variant("v1.4 + BACA", &p_v14_baca,
        Some(wiring(Some(BaceAuthentication::new(key, "bench")))));

    // A single criterion sample so the bench is a "scorecard" rather than a
    // timing distribution — the per-variant numbers above are what matters.
    c.bench_function("dhis2_like_scorecard", |b| {
        b.iter(|| { /* workload executed once above; nothing to measure here */ });
    });
}

criterion_group!(benches, dhis2_like_variants);
criterion_main!(benches);
