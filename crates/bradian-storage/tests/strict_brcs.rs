//! ADR 0092 — strict-authenticated mode on `.brcs` trailers.

use std::sync::Arc;

use bradian_relational::bace::authenticated::BaceAuthentication;
use bradian_relational::bace::codec::raw::RAW_ID;
use bradian_relational::bace::codec::zstd::ZSTD_ID;
use bradian_relational::bace::policy::{
    CompressionProfile, InMemoryPolicyService, PlatformBaseline,
};
use bradian_relational::bace::registry::BuiltInRegistry;
use bradian_relational::row_store::BaceWiring;
use bradian_core::{Column, LogicalType, Schema, Value};
use bradian_storage::{SegmentReader, SegmentWriter, SegmentWriterOptions};
use tempfile::tempdir;

fn wiring(auth: Option<BaceAuthentication>, strict: bool) -> BaceWiring {
    let baseline = PlatformBaseline {
        profile: CompressionProfile::Balanced,
        allowed_codecs: vec![ZSTD_ID, RAW_ID],
        ..PlatformBaseline::shipped()
    };
    BaceWiring {
        policy_service: Arc::new(InMemoryPolicyService::new(baseline, BuiltInRegistry::new())),
        registry: Arc::new(BuiltInRegistry::new()),
        authentication: auth,
        strict_authenticated: strict,
        previous_authentication: None,
    }
}

fn opts() -> SegmentWriterOptions {
    SegmentWriterOptions {
        dataset_id: "d".into(), partition_id: "p".into(), segment_id: "s".into(),
        compress: true, dict_cardinality_threshold: 0.5,
        watermark_ms: 0, data_revision: 0, metadata_revision: None,
        max_rows_per_page: None,
        use_column_dict: false,
    }
}

fn schema() -> Schema {
    Schema::new(vec![Column::new("v", LogicalType::I64).required()]).unwrap()
}

// ---------- §3 item 3 — strict rejects .brcs trailer 1 (BACE) ----------

#[test]
fn item_3_strict_rejects_brcs_trailer_1() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.brcs");
    // Write under non-auth BACE wiring → trailer 1 pages.
    let mut w = SegmentWriter::new_with_bace(schema(), opts(), wiring(None, false));
    for i in 0..10 { w.append_row(&[Value::I64(i)]).unwrap(); }
    w.finish_to_path(&path).unwrap();

    let mut r = SegmentReader::open_with_bace(
        &path, wiring(Some(BaceAuthentication::new([3u8; 32], "k")), true),
    ).unwrap();
    let err = r.read_column("v").unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("strict mode"), "expected strict rejection; got {msg}");
}

// ---------- §3 item 4 — strict accepts .brcs trailer 2 (BACA) ----------

#[test]
fn item_4_strict_accepts_brcs_trailer_2() {
    let key = [7u8; 32];
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.brcs");
    let mut w = SegmentWriter::new_with_bace(
        schema(), opts(),
        wiring(Some(BaceAuthentication::new(key, "w")), false),
    );
    for i in 0..10 { w.append_row(&[Value::I64(i)]).unwrap(); }
    w.finish_to_path(&path).unwrap();

    let mut r = SegmentReader::open_with_bace(
        &path, wiring(Some(BaceAuthentication::new(key, "w")), true),
    ).unwrap();
    let col = r.read_column("v").unwrap();
    assert_eq!(col.len(), 10);
}

// ---------- §3 item 2 — strict leaves legacy v1.0 segments readable ----------

#[test]
fn item_2_strict_tolerates_legacy_v1_0_segment() {
    // Honest boundary: pre-BACE segments carry no trailer byte,
    // so strict mode cannot "downgrade-reject" them. Reading
    // them under strict + auth succeeds. Operators who want to
    // reject legacy segments entirely should check the minor
    // version / BACE_PAGE_ENVELOPE flag before opening.
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.brcs");
    let mut w = SegmentWriter::new(schema(), opts());
    for i in 0..10 { w.append_row(&[Value::I64(i)]).unwrap(); }
    w.finish_to_path(&path).unwrap();

    let mut r = SegmentReader::open_with_bace(
        &path, wiring(Some(BaceAuthentication::new([3u8; 32], "k")), true),
    ).unwrap();
    let col = r.read_column("v").unwrap();
    assert_eq!(col.len(), 10);
}

// ---------- §3 item 1 — non-strict preserves pre-ADR-0092 behavior ----------

#[test]
fn item_1_non_strict_tolerates_trailer_1() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.brcs");
    let mut w = SegmentWriter::new_with_bace(schema(), opts(), wiring(None, false));
    for i in 0..10 { w.append_row(&[Value::I64(i)]).unwrap(); }
    w.finish_to_path(&path).unwrap();

    // Auth wiring, strict=false → trailer 1 still readable.
    let mut r = SegmentReader::open_with_bace(
        &path, wiring(Some(BaceAuthentication::new([3u8; 32], "k")), false),
    ).unwrap();
    let col = r.read_column("v").unwrap();
    assert_eq!(col.len(), 10);
}
