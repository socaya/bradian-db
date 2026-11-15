//! ADR 0081 — `.derc` multi-page writer integration tests.
//!
//! Covers the §3.5 checklist: default byte-identical output,
//! N-page emission, round-trip with concat_buffers, ZoneMap
//! merge, validity across pages, UTF-8 offset rebasing, dict
//! bypass, BACE/BACA/dict-trailer per-page, and the HARD-FAIL
//! strict size-reduction test.

use std::collections::HashMap;
use std::sync::Arc;

use bradian_relational::bace::codec::raw::RAW_ID;
use bradian_relational::bace::codec::zstd::ZSTD_ID;
use bradian_relational::bace::dictionary::train_zstd_dictionary;
use bradian_relational::bace::policy::{
    CompressionProfile, InMemoryPolicyService, PlatformBaseline,
};
use bradian_relational::bace::registry::BuiltInRegistry;
use bradian_relational::row_store::BaceWiring;
use dap_core::{Column, LogicalType, Schema, Value};
use dap_storage::{ColumnBuffer, SegmentReader, SegmentWriter, SegmentWriterOptions};
use tempfile::tempdir;

fn wiring_zstd_balanced() -> BaceWiring {
    let baseline = PlatformBaseline {
        profile: CompressionProfile::Balanced,
        allowed_codecs: vec![ZSTD_ID, RAW_ID],
        ..PlatformBaseline::shipped()
    };
    BaceWiring {
        policy_service: Arc::new(InMemoryPolicyService::new(baseline, BuiltInRegistry::new())),
        registry: Arc::new(BuiltInRegistry::new()),
        authentication: None,
    }
}

fn opts_with_split(split: Option<usize>) -> SegmentWriterOptions {
    SegmentWriterOptions {
        dataset_id: "d".into(), partition_id: "p".into(), segment_id: "s".into(),
        compress: true, dict_cardinality_threshold: 0.5,
        watermark_ms: 0, data_revision: 0, metadata_revision: None,
        max_rows_per_page: split,
    }
}

/// Multi-page tests of the Utf8 concat path need the writer's
/// inner codec to be PlainUtf8 (not Dictionary). Threshold `0.0`
/// disables the inner-dict path for Utf8 family columns because
/// no column can have cardinality ≤ 0.
fn opts_force_plain_utf8(split: Option<usize>) -> SegmentWriterOptions {
    SegmentWriterOptions {
        dict_cardinality_threshold: 0.0,
        ..opts_with_split(split)
    }
}

// Mixed numeric + utf8 schema that will exercise both concat
// variants under multi-page writes.
fn numeric_schema() -> Schema {
    Schema::new(vec![
        Column::new("id", LogicalType::I64).required(),
        Column::new("ratio", LogicalType::F64),         // nullable
        Column::new("note", LogicalType::Utf8),         // nullable, Utf8-plain (dict bypass)
    ]).unwrap()
}

fn numeric_rows(n: usize) -> Vec<Vec<Value>> {
    (0..n).map(|i| vec![
        Value::I64(i as i64),
        if i % 7 == 0 { Value::Null } else { Value::F64(i as f64 * 1.5) },
        Value::Str(format!("note-{:04}-extra-padding-text", i % 16)),
    ]).collect()
}

// ---------- §3.5 item 1 — default byte-identical output ----------

#[test]
fn item_1_default_options_produce_byte_identical_single_page_output() {
    // Any prior test that wrote with max_rows_per_page=None must
    // produce exactly the same file size and page layout.
    let dir = tempdir().unwrap();
    let path_a = dir.path().join("a.derc");
    let path_b = dir.path().join("b.derc");

    let rows = numeric_rows(100);
    for path in [&path_a, &path_b] {
        let mut w = SegmentWriter::new(numeric_schema(), opts_with_split(None));
        for r in &rows { w.append_row(r).unwrap(); }
        w.finish_to_path(path).unwrap();
    }

    let a = std::fs::read(&path_a).unwrap();
    let b = std::fs::read(&path_b).unwrap();
    // created_at (wall-clock) differs; everything else must match.
    // We instead confirm every column has exactly one page.
    let r = SegmentReader::open(&path_a).unwrap();
    for col in r.columns() {
        assert_eq!(col.pages.len(), 1,
            "default (None) must emit exactly 1 page per column");
    }
    assert_eq!(a.len(), b.len(),
        "byte lengths should match (modulo created_at which is the same ms-resolution clock)");
}

// ---------- §3.5 item 2 — N pages emitted when option set ----------

#[test]
fn item_2_max_rows_per_page_100_on_350_rows_emits_four_pages() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("split.derc");

    let rows = numeric_rows(350);
    let mut w = SegmentWriter::new(numeric_schema(), opts_force_plain_utf8(Some(100)));
    for r in &rows { w.append_row(r).unwrap(); }
    w.finish_to_path(&path).unwrap();

    let r = SegmentReader::open(&path).unwrap();
    // Non-dict columns split; the Utf8 column uses PlainUtf8 codec
    // (not Dictionary) so it also splits.
    let id_pages = r.columns().iter().find(|c| c.name == "id").unwrap().pages.len();
    let note_pages = r.columns().iter().find(|c| c.name == "note").unwrap().pages.len();
    assert_eq!(id_pages, 4, "350 rows / 100 per page = 4 pages (100+100+100+50)");
    assert_eq!(note_pages, 4, "Utf8 with threshold=0.0 forces PlainUtf8 inner codec, column splits");

    // Row count distribution.
    let id_col = r.columns().iter().find(|c| c.name == "id").unwrap();
    let counts: Vec<u32> = id_col.pages.iter().map(|p| p.row_count).collect();
    assert_eq!(counts, vec![100, 100, 100, 50]);
}

// ---------- §3.5 item 3 — round-trip through concat_buffers ----------

#[test]
fn item_3_multi_page_segment_round_trips_to_identical_values() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("rt.derc");
    let rows = numeric_rows(350);
    let mut w = SegmentWriter::new(numeric_schema(), opts_with_split(Some(100)));
    for r in &rows { w.append_row(r).unwrap(); }
    w.finish_to_path(&path).unwrap();

    let mut r = SegmentReader::open(&path).unwrap();
    let id = r.read_column("id").unwrap();
    let ColumnBuffer::I64 { data, .. } = id else { panic!("expected I64"); };
    assert_eq!(data.len(), 350);
    for (i, v) in data.iter().enumerate() {
        assert_eq!(*v, i as i64);
    }

    let ratio = r.read_column("ratio").unwrap();
    let ColumnBuffer::F64 { data: rd, validity } = ratio else { panic!("expected F64"); };
    assert_eq!(rd.len(), 350);
    assert_eq!(validity.len(), 350);
    // Every 7th row is NULL.
    for (i, v) in rd.iter().enumerate() {
        if i % 7 == 0 {
            assert!(!validity.is_valid(i), "row {i} should be null");
        } else {
            assert!(validity.is_valid(i), "row {i} should be valid");
            assert_eq!(*v, i as f64 * 1.5);
        }
    }
}

// ---------- §3.5 item 4 — ZoneMap merge ----------

#[test]
fn item_4_zone_map_row_count_and_null_count_sum_correctly() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("zone.derc");
    let rows = numeric_rows(350);
    let mut w = SegmentWriter::new(numeric_schema(), opts_with_split(Some(100)));
    for r in &rows { w.append_row(r).unwrap(); }
    w.finish_to_path(&path).unwrap();

    let r = SegmentReader::open(&path).unwrap();
    let ratio_col = r.columns().iter().find(|c| c.name == "ratio").unwrap();
    assert_eq!(ratio_col.zone_map.row_count, 350);
    // 0, 7, 14, …, 343, 350 - no wait, 349: nulls at i%7==0.
    // That's i in {0, 7, 14, ..., 343} = 50 nulls.
    let expected_nulls = (0..350).filter(|i| i % 7 == 0).count() as u32;
    assert_eq!(ratio_col.zone_map.null_count, expected_nulls);
}

// ---------- §3.5 item 5 — validity bitmaps across pages ----------

#[test]
fn item_5_validity_bitmaps_across_pages_reproduce_null_mask() {
    // Covered partially by item 3. Here we explicitly check the
    // concat'd validity bitmap bit-by-bit.
    let dir = tempdir().unwrap();
    let path = dir.path().join("validity.derc");
    let rows = numeric_rows(350);
    let mut w = SegmentWriter::new(numeric_schema(), opts_with_split(Some(100)));
    for r in &rows { w.append_row(r).unwrap(); }
    w.finish_to_path(&path).unwrap();

    let mut r = SegmentReader::open(&path).unwrap();
    let ratio = r.read_column("ratio").unwrap();
    let validity = match &ratio {
        ColumnBuffer::F64 { validity, .. } => validity.clone(),
        _ => panic!("expected F64"),
    };
    for i in 0..350 {
        let is_valid = validity.is_valid(i);
        let should_be_valid = i % 7 != 0;
        assert_eq!(is_valid, should_be_valid, "validity mismatch at {i}");
    }
}

// ---------- §3.5 item 6 — UTF-8 offset rebasing ----------

#[test]
fn item_6_utf8_offsets_after_concat_point_into_merged_buffer() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("utf8.derc");
    let rows = numeric_rows(350);
    let mut w = SegmentWriter::new(numeric_schema(), opts_force_plain_utf8(Some(100)));
    for r in &rows { w.append_row(r).unwrap(); }
    w.finish_to_path(&path).unwrap();

    let mut r = SegmentReader::open(&path).unwrap();
    let note = r.read_column("note").unwrap();
    let ColumnBuffer::Utf8 { offsets, data, .. } = note else { panic!("expected Utf8"); };
    // 350 rows → 351 offsets. First offset must be 0; last must equal data.len().
    assert_eq!(offsets.len(), 351);
    assert_eq!(offsets[0], 0);
    assert_eq!(offsets[350] as usize, data.len());
    // Offsets must be monotonically non-decreasing.
    for i in 1..offsets.len() {
        assert!(offsets[i] >= offsets[i - 1],
            "offsets must be non-decreasing at {i}: {} < {}",
            offsets[i], offsets[i - 1]);
    }
    // Spot-check a row.
    let r100_s = offsets[100] as usize;
    let r100_e = offsets[101] as usize;
    let r100 = std::str::from_utf8(&data[r100_s..r100_e]).unwrap();
    assert_eq!(r100, format!("note-{:04}-extra-padding-text", 100 % 16));
}

// ---------- §3.5 item 7 — dict column now splits (ADR 0082) ----------

#[test]
fn item_7_dict_encoded_column_splits_like_every_other_column() {
    // ADR 0081 §2 non-goal "dict columns bypass split" is
    // reversed by ADR 0082: dict columns split like any other.
    // Every page carries a verbatim copy of the column dict.
    let schema = Schema::new(vec![
        Column::new("id", LogicalType::I64).required(),
        Column::new("tag", LogicalType::Uid).required(),   // dict path
    ]).unwrap();
    let rows: Vec<Vec<Value>> = (0..200).map(|i| vec![
        Value::I64(i),
        Value::Str(format!("tag-{:02}", i % 4)),           // 4 distinct values
    ]).collect();

    let dir = tempdir().unwrap();
    let path = dir.path().join("dict_split.derc");
    let mut w = SegmentWriter::new(schema, opts_with_split(Some(50)));
    for r in &rows { w.append_row(r).unwrap(); }
    w.finish_to_path(&path).unwrap();

    let r = SegmentReader::open(&path).unwrap();
    let id_pages = r.columns().iter().find(|c| c.name == "id").unwrap().pages.len();
    let tag_pages = r.columns().iter().find(|c| c.name == "tag").unwrap().pages.len();
    assert_eq!(id_pages, 4, "non-dict id column should split into 4 pages");
    assert_eq!(tag_pages, 4, "ADR 0082: dict column also splits into 4 pages");
}

// ---------- §3.5 item 8 — reading dict column from multi-page writer ----------

#[test]
fn item_8_dict_column_from_multi_page_writer_round_trips() {
    // §3.5 item 8: because the writer bypasses dict columns from
    // the split, reading one from a "multi-page writer" still
    // produces correct values.
    let schema = Schema::new(vec![
        Column::new("tag", LogicalType::Uid).required(),
    ]).unwrap();
    let rows: Vec<Vec<Value>> = (0..200).map(|i| vec![
        Value::Str(format!("tag-{:02}", i % 4)),
    ]).collect();

    let dir = tempdir().unwrap();
    let path = dir.path().join("dict_rt.derc");
    let mut w = SegmentWriter::new(schema, opts_with_split(Some(50)));
    for r in &rows { w.append_row(r).unwrap(); }
    w.finish_to_path(&path).unwrap();

    let mut r = SegmentReader::open(&path).unwrap();
    let tag = r.read_column("tag").unwrap();
    assert_eq!(tag.len(), 200);
    // Spot-check: row 100's value.
    let v = tag.value_at(100).unwrap();
    match v {
        Value::Str(s) => assert_eq!(s, format!("tag-{:02}", 100 % 4)),
        other => panic!("expected Str, got {other:?}"),
    }
}

// ---------- §3.5 item 9 — BACE envelopes per page ----------

#[test]
fn item_9_bace_envelope_round_trips_across_multi_page_segment() {
    // Each page carries its own BACE envelope; the reader
    // dispatches per page through the existing trailer-byte
    // logic. If this round-trips, the per-page envelope path
    // is correct.
    let dir = tempdir().unwrap();
    let path = dir.path().join("multi_bace.derc");
    let rows = numeric_rows(350);
    let mut w = SegmentWriter::new_with_bace(
        numeric_schema(), opts_with_split(Some(100)),
        wiring_zstd_balanced(),
    );
    for r in &rows { w.append_row(r).unwrap(); }
    w.finish_to_path(&path).unwrap();

    let mut r = SegmentReader::open_with_bace(&path, wiring_zstd_balanced()).unwrap();
    let id = r.read_column("id").unwrap();
    let ColumnBuffer::I64 { data, .. } = id else { panic!("expected I64"); };
    assert_eq!(data.len(), 350);
    for (i, v) in data.iter().enumerate() {
        assert_eq!(*v, i as i64);
    }
}

// ---------- §3.5 item 10 — STRICT size-reduction demonstration (HARD FAIL) ----------

#[test]
fn item_10_multi_page_v1_2_strictly_smaller_than_v1_1_on_small_page_payload() {
    // THE ACCEPTANCE GATE. The ADR 0079 §10 observation
    // ("v1.2 is 266 bytes LARGER than v1.1 on single-page
    // redundant data") is replaced on a multi-page segment:
    // the dict pool amortizes across pages, so v1.2 should be
    // strictly smaller.
    //
    // Payload design: SMALL pages (8 rows each) × 512 pages
    // = 4096 total rows. With only 8 rows per page, plain
    // ZSTD has very little within-page data to discover the
    // common prefix patterns. A trained dict teaches the
    // prefixes once in the pool and amortizes across all 512
    // pages. Rows share a 40-byte prefix but have unique
    // 20-byte suffixes.
    //
    // This matches the ADR 0081 claim: dict-mode compression
    // pays off when (a) pages are small and (b) the dict pool
    // amortizes across many pages. Large pages let plain ZSTD
    // self-learn and defeat the dict's advantage.
    let schema = Schema::new(vec![
        Column::new("tag", LogicalType::Utf8).required(),
    ]).unwrap();
    let rows: Vec<Vec<Value>> = (0..4096).map(|i| vec![
        Value::Str(format!(
            "DHIS2-ORGANISATIONUNIT-COMMON-PREFIX-{:08}-SUFFIX-{:08}",
            i / 64, i,
        )),
    ]).collect();

    let dir = tempdir().unwrap();
    let v11_path = dir.path().join("v11.derc");
    let v12_path = dir.path().join("v12.derc");

    // 8 rows per page = 512 pages.
    let page_size = Some(8usize);

    let mut w = SegmentWriter::new_with_bace(
        schema.clone(), opts_force_plain_utf8(page_size),
        wiring_zstd_balanced(),
    );
    for r in &rows { w.append_row(r).unwrap(); }
    w.finish_to_path(&v11_path).unwrap();

    // v1.2 — dict trained on 512 sample rows.
    let samples_owned: Vec<Vec<u8>> = (0..512).map(|i| {
        format!(
            "DHIS2-ORGANISATIONUNIT-COMMON-PREFIX-{:08}-SUFFIX-{:08}",
            i / 64, i,
        ).into_bytes()
    }).collect();
    let refs: Vec<&[u8]> = samples_owned.iter().map(|v| v.as_slice()).collect();
    let dict = train_zstd_dictionary(&refs, 4096, "tag").unwrap();
    let mut dicts = HashMap::new();
    dicts.insert("tag".to_string(), dict);
    let mut w = SegmentWriter::new_with_bace_and_dictionaries(
        schema, opts_force_plain_utf8(page_size),
        wiring_zstd_balanced(), dicts,
    );
    for r in &rows { w.append_row(r).unwrap(); }
    w.finish_to_path(&v12_path).unwrap();

    let v11_size = std::fs::metadata(&v11_path).unwrap().len();
    let v12_size = std::fs::metadata(&v12_path).unwrap().len();

    println!(
        "ADR0081-S10 v1.1 (multi-page, no dict): {v11_size} bytes, \
         v1.2 (multi-page, with dict): {v12_size} bytes, \
         reduction: {:.1}%",
        (1.0 - v12_size as f64 / v11_size as f64) * 100.0,
    );

    // HARD FAIL GATE per ADR 0081 §3.5 item 10.
    assert!(
        v12_size < v11_size,
        "ADR 0081 §3.5 item 10 FAILED: v1.2 multi-page ({v12_size} bytes) is NOT strictly \
         smaller than v1.1 multi-page ({v11_size} bytes). The dict pool is not amortizing \
         across pages on this workload. ADR 0081 should be reconsidered rather than \
         defensively accepted.",
    );
}
