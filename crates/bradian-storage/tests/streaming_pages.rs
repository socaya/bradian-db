//! ADR 0088 — streaming page iterator for `.brcs` columns.
//!
//! §3.5 checklist: correct page count, per-page row counts,
//! concat equals read_column, iterator stops on error, v1.4
//! and non-dict shapes both yield standalone pages.

use std::sync::Arc;

use bradian_relational::bace::codec::raw::RAW_ID;
use bradian_relational::bace::codec::zstd::ZSTD_ID;
use bradian_relational::bace::policy::{
    CompressionProfile, InMemoryPolicyService, PlatformBaseline,
};
use bradian_relational::bace::registry::BuiltInRegistry;
use bradian_relational::row_store::BaceWiring;
use bradian_core::{Column, LogicalType, Schema, Value};
use bradian_storage::{ColumnBuffer, SegmentReader, SegmentWriter, SegmentWriterOptions};
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
    }
}

fn opts(split: Option<usize>, column_dict: bool) -> SegmentWriterOptions {
    SegmentWriterOptions {
        dataset_id: "d".into(), partition_id: "p".into(), segment_id: "s".into(),
        compress: true, dict_cardinality_threshold: 0.5,
        watermark_ms: 0, data_revision: 0, metadata_revision: None,
        max_rows_per_page: split,
        use_column_dict: column_dict,
    }
}

// ---------- §3.5 item 1 — page count matches ----------

#[test]
fn item_1_iter_pages_yields_exact_page_count() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("count.brcs");
    let schema = Schema::new(vec![
        Column::new("id", LogicalType::I64).required(),
    ]).unwrap();
    let mut w = SegmentWriter::new(schema, opts(Some(25), false));
    for i in 0..200 { w.append_row(&[Value::I64(i)]).unwrap(); }
    w.finish_to_path(&path).unwrap();

    let mut r = SegmentReader::open(&path).unwrap();
    let pages: Vec<_> = r.iter_pages("id").unwrap().collect();
    assert_eq!(pages.len(), 8, "200 rows / 25 per page = 8 pages");
    for page in &pages {
        assert!(page.is_ok());
    }
}

// ---------- §3.5 item 2 — per-page row counts ----------

#[test]
fn item_2_per_page_row_counts_match_declared() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("rowcount.brcs");
    let schema = Schema::new(vec![
        Column::new("id", LogicalType::I64).required(),
    ]).unwrap();
    let mut w = SegmentWriter::new(schema, opts(Some(25), false));
    for i in 0..150 { w.append_row(&[Value::I64(i)]).unwrap(); }
    w.finish_to_path(&path).unwrap();

    let mut r = SegmentReader::open(&path).unwrap();
    let col = r.column("id").unwrap().clone();
    let declared: Vec<_> = col.pages.iter().map(|p| p.row_count as usize).collect();
    let actual: Vec<_> = r.iter_pages("id").unwrap()
        .map(|p| p.unwrap().len())
        .collect();
    assert_eq!(declared, actual);
}

// ---------- §3.5 item 3 — concat equals read_column ----------

#[test]
fn item_3_streaming_concat_equals_read_column_output() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("concat_eq.brcs");
    let schema = Schema::new(vec![
        Column::new("id", LogicalType::I64).required(),
        Column::new("note", LogicalType::Utf8),
    ]).unwrap();
    let mut w = SegmentWriter::new(schema, opts(Some(40), false));
    for i in 0..200 {
        w.append_row(&[
            Value::I64(i),
            Value::Str(format!("row-{i}")),
        ]).unwrap();
    }
    w.finish_to_path(&path).unwrap();

    let mut r1 = SegmentReader::open(&path).unwrap();
    let whole = r1.read_column("note").unwrap();
    let mut r2 = SegmentReader::open(&path).unwrap();
    let pages: Vec<ColumnBuffer> = r2.iter_pages("note").unwrap()
        .map(|p| p.unwrap()).collect();

    // Concat yielded pages manually and compare row-by-row.
    let mut streamed_values: Vec<Value> = Vec::new();
    for p in &pages {
        for i in 0..p.len() {
            streamed_values.push(p.value_at(i).unwrap());
        }
    }
    assert_eq!(whole.len(), streamed_values.len());
    for (i, sv) in streamed_values.iter().enumerate() {
        assert_eq!(&whole.value_at(i).unwrap(), sv);
    }
}

// ---------- §3.5 item 5 — v1.4 dict column pages ----------

#[test]
fn item_5_v14_dict_column_pages_are_dictindex32() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("v14_stream.brcs");
    let schema = Schema::new(vec![
        Column::new("tag", LogicalType::Uid).required(),
    ]).unwrap();
    let mut w = SegmentWriter::new(schema, opts(Some(50), true));
    for i in 0..200 { w.append_row(&[Value::Str(format!("t-{:04}", i % 4))]).unwrap(); }
    w.finish_to_path(&path).unwrap();

    let mut r = SegmentReader::open_with_bace(&path, wiring()).unwrap();
    let mut n = 0;
    for p in r.iter_pages("tag").unwrap() {
        let buf = p.unwrap();
        assert!(matches!(buf, ColumnBuffer::DictIndex32 { .. }),
            "every v1.4 page must yield DictIndex32");
        n += 1;
    }
    assert_eq!(n, 4);
}

// ---------- §3.5 item 6 — non-dict I64 pages yield I64 ----------

#[test]
fn item_6_non_dict_pages_yield_expected_variants() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("non_dict_stream.brcs");
    let schema = Schema::new(vec![
        Column::new("v", LogicalType::F64),
    ]).unwrap();
    let mut w = SegmentWriter::new(schema, opts(Some(10), false));
    for i in 0..30 { w.append_row(&[Value::F64(i as f64)]).unwrap(); }
    w.finish_to_path(&path).unwrap();

    let mut r = SegmentReader::open(&path).unwrap();
    for p in r.iter_pages("v").unwrap() {
        assert!(matches!(p.unwrap(), ColumnBuffer::F64 { .. }));
    }
}

// ---------- §3.5 item 7 — zero-page column edge ----------

#[test]
fn item_7_single_page_column_yields_one_page() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("single.brcs");
    let schema = Schema::new(vec![
        Column::new("v", LogicalType::I64).required(),
    ]).unwrap();
    let mut w = SegmentWriter::new(schema, opts(None, false));
    for i in 0..5 { w.append_row(&[Value::I64(i)]).unwrap(); }
    w.finish_to_path(&path).unwrap();

    let mut r = SegmentReader::open(&path).unwrap();
    let pages: Vec<_> = r.iter_pages("v").unwrap().collect();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].as_ref().unwrap().len(), 5);
}

// ---------- §3.5 item 8 — reader usable after abandoning iterator ----------

#[test]
fn item_8_reader_usable_after_abandoning_iterator() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("abandon.brcs");
    let schema = Schema::new(vec![
        Column::new("v", LogicalType::I64).required(),
    ]).unwrap();
    let mut w = SegmentWriter::new(schema, opts(Some(10), false));
    for i in 0..30 { w.append_row(&[Value::I64(i)]).unwrap(); }
    w.finish_to_path(&path).unwrap();

    let mut r = SegmentReader::open(&path).unwrap();
    // Take just one page and drop the iterator.
    {
        let mut it = r.iter_pages("v").unwrap();
        let _ = it.next().unwrap().unwrap();
    }
    // Reader must still work.
    let whole = r.read_column("v").unwrap();
    assert_eq!(whole.len(), 30);
}

// ---------- §3.5 item 10 — unknown column name returns NotFound ----------

#[test]
fn item_10_unknown_column_name_errors_before_iteration() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("unknown.brcs");
    let schema = Schema::new(vec![
        Column::new("v", LogicalType::I64).required(),
    ]).unwrap();
    let mut w = SegmentWriter::new(schema, opts(None, false));
    w.append_row(&[Value::I64(1)]).unwrap();
    w.finish_to_path(&path).unwrap();

    let mut r = SegmentReader::open(&path).unwrap();
    let err = r.iter_pages("nonexistent").unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("nonexistent") || msg.contains("NotFound"),
        "expected NotFound; got: {msg}");
}
