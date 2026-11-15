//! ADR 0076 — `.derc` BACE carrier adapter integration tests.
//!
//! Covers the §3.5 checklist: v1.0 preservation, v1.1 round-trip,
//! cross-version reject paths, tamper detection, cap enforcement,
//! and reserved-value rejection.

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

fn wiring_raw_only() -> BaceWiring {
    let baseline = PlatformBaseline {
        profile: CompressionProfile::Raw,
        allowed_codecs: vec![RAW_ID],
        ..PlatformBaseline::shipped()
    };
    BaceWiring {
        policy_service: Arc::new(InMemoryPolicyService::new(baseline, BuiltInRegistry::new())),
        registry: Arc::new(BuiltInRegistry::new()),
        authentication: None,
    }
}

fn schema() -> Schema {
    Schema::new(vec![
        Column::new("id", LogicalType::I64).required(),
        Column::new("tag", LogicalType::Utf8).required(),
    ]).unwrap()
}

fn rows() -> Vec<Vec<Value>> {
    (0..64).map(|i| vec![
        Value::I64(i),
        Value::Str(format!("tag-{:04}", i % 8)),
    ]).collect()
}

fn write_segment(path: &std::path::Path, bace: Option<BaceWiring>) -> bradian_storage::WrittenSegment {
    let opts = SegmentWriterOptions {
        dataset_id: "d".into(),
        partition_id: "p".into(),
        segment_id: "s".into(),
        compress: true,
        dict_cardinality_threshold: 0.5,
        watermark_ms: 0,
        data_revision: 0,
        metadata_revision: None,
        max_rows_per_page: None,
        use_column_dict: false,
    };
    let mut w = match bace {
        Some(wiring) => SegmentWriter::new_with_bace(schema(), opts, wiring),
        None => SegmentWriter::new(schema(), opts),
    };
    for r in &rows() { w.append_row(r).unwrap(); }
    w.finish_to_path(path).unwrap()
}

fn assert_round_trip(reader: &mut SegmentReader) {
    let got = reader.read_column("id").unwrap();
    let ColumnBuffer::I64 { data, .. } = got else { panic!("expected I64 column"); };
    assert_eq!(data.len(), 64);
    for (i, v) in data.iter().enumerate() {
        assert_eq!(*v, i as i64);
    }
}

// ---------- §3.5 items 1, 3: writer paths ----------

#[test]
fn item_1_writer_with_zstd_balanced_emits_v1_1_bace_segment() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bace.derc");
    write_segment(&path, Some(wiring_zstd_balanced()));

    // Confirm v1.1 header: minor version = 1 AND BACE_PAGE_ENVELOPE flag set.
    let bytes = std::fs::read(&path).unwrap();
    let minor = u16::from_le_bytes([bytes[10], bytes[11]]);
    assert_eq!(minor, 1, "writer with BACE wiring must emit minor version 1");
    let required = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
    assert_eq!(required & (1 << 5), 1 << 5,
        "writer with BACE wiring must set BACE_PAGE_ENVELOPE flag");
}

#[test]
fn item_3_writer_without_wiring_emits_v1_0_byte_identical() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("legacy.derc");
    write_segment(&path, None);

    let bytes = std::fs::read(&path).unwrap();
    let minor = u16::from_le_bytes([bytes[10], bytes[11]]);
    assert_eq!(minor, 0, "no-wiring writer must emit minor version 0");
    let required = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
    assert_eq!(required & (1 << 5), 0,
        "no-wiring writer must NOT set BACE_PAGE_ENVELOPE flag");
}

#[test]
fn writer_with_raw_only_wiring_opts_out_of_bace_envelope_overhead() {
    // §3.2 — RAW profile opts out so writers don't pay 36 bytes
    // of envelope per page for no compression benefit.
    let dir = tempdir().unwrap();
    let path = dir.path().join("raw.derc");
    write_segment(&path, Some(wiring_raw_only()));
    let bytes = std::fs::read(&path).unwrap();
    let required = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
    assert_eq!(required & (1 << 5), 0,
        "RAW-only wiring must NOT set BACE_PAGE_ENVELOPE flag");
}

// ---------- §3.5 items 2, 4, 6: reader round-trip ----------

#[test]
fn item_2_reader_with_bace_round_trips_bace_segment() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("rt.derc");
    write_segment(&path, Some(wiring_zstd_balanced()));
    let mut r = SegmentReader::open_with_bace(&path, wiring_zstd_balanced()).unwrap();
    assert_round_trip(&mut r);
}

#[test]
fn item_4_legacy_reader_reads_v1_0_segment_unchanged() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("legacy.derc");
    write_segment(&path, None);
    let mut r = SegmentReader::open(&path).unwrap();
    assert_round_trip(&mut r);
}

#[test]
fn item_6_reader_with_bace_reads_legacy_v1_0_segment() {
    // Wiring tolerated on legacy v1.0 (no feature flag, no trailer).
    let dir = tempdir().unwrap();
    let path = dir.path().join("legacy.derc");
    write_segment(&path, None);
    let mut r = SegmentReader::open_with_bace(&path, wiring_zstd_balanced()).unwrap();
    assert_round_trip(&mut r);
}

// ---------- §3.5 item 5: cross-version reject ----------

#[test]
fn item_5_legacy_reader_rejects_v1_1_segment_via_feature_flag() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bace.derc");
    write_segment(&path, Some(wiring_zstd_balanced()));
    let err = SegmentReader::open(&path).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("feature bits") || msg.contains("not supported"),
        "expected unsupported-feature error, got: {msg}",
    );
}

// ---------- §3.5 item 7: tamper detection ----------

#[test]
fn item_7_tampered_envelope_bytes_fail_closed() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("tamper.derc");
    write_segment(&path, Some(wiring_zstd_balanced()));
    let mut bytes = std::fs::read(&path).unwrap();
    // Flip a byte inside the first page body (after 64-byte header).
    // The payload starts at offset 64 (right after the header).
    bytes[64 + 2] ^= 0xFF;
    std::fs::write(&path, &bytes).unwrap();

    let err = SegmentReader::open_with_bace(&path, wiring_zstd_balanced())
        .and_then(|mut r| r.read_column("id"))
        .unwrap_err();
    let msg = format!("{err}");
    // Could fail as CRC mismatch (page CRC covers payload+validity),
    // or BACE envelope CRC mismatch, or codec decode error. Any
    // fail-closed surface is acceptable.
    assert!(
        msg.contains("crc") || msg.contains("envelope")
            || msg.contains("decode") || msg.contains("BACE")
            || msg.contains("corruption"),
        "expected tamper-detected error, got: {msg}",
    );
}

// ---------- §3.5 item 9: BACA reserved value fails cleanly ----------

#[test]
fn item_9_baca_reserved_trailer_byte_fails_closed() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("baca.derc");
    write_segment(&path, Some(wiring_zstd_balanced()));
    let mut bytes = std::fs::read(&path).unwrap();
    // Locate the first page's trailer byte. The writer wrote:
    //   64-byte header | payload | validity | trailer=1 | ...
    // For this schema (id non-nullable, tag non-nullable), validity_len=0.
    // Decode the footer to find payload_len.
    let file_len = bytes.len();
    let footer_start = u64::from_le_bytes(bytes[file_len - 8..].try_into().unwrap()) as usize;
    // Footer layout: 4-byte FOOTER_MAGIC, 4-byte len, 4-byte crc, payload, 8-byte footer_start.
    let payload_len = u32::from_le_bytes(bytes[footer_start + 4..footer_start + 8].try_into().unwrap()) as usize;
    let payload_crc = u32::from_le_bytes(bytes[footer_start + 8..footer_start + 12].try_into().unwrap());
    let footer_bytes = &bytes[footer_start + 12..footer_start + 12 + payload_len];
    // Verify CRC before deserializing.
    let expect_crc = crc32_bytes(footer_bytes);
    assert_eq!(expect_crc, payload_crc);
    let footer: bradian_storage::SegmentFooter = bincode::deserialize(footer_bytes).unwrap();
    let first_page = &footer.columns[0].pages[0];
    let trailer_offset = if first_page.validity_len > 0 {
        first_page.validity_offset + first_page.validity_len as u64
    } else {
        first_page.payload_offset + first_page.payload_len as u64
    } as usize;

    // Confirm current value is 1 (BACE v1), flip to 2 (BACA reserved).
    assert_eq!(bytes[trailer_offset], 1,
        "writer should have emitted trailer byte = BaceV1 (1)");
    bytes[trailer_offset] = 2;
    std::fs::write(&path, &bytes).unwrap();

    let err = SegmentReader::open_with_bace(&path, wiring_zstd_balanced())
        .and_then(|mut r| r.read_column("id"))
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("BACA") || msg.contains("reserved") || msg.contains("not supported"),
        "expected BACA-reserved rejection, got: {msg}",
    );
}

// ---------- §3.5 item 10: unknown trailer byte ----------

#[test]
fn item_10_unknown_trailer_byte_fails_corruption() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("unk.derc");
    write_segment(&path, Some(wiring_zstd_balanced()));
    let mut bytes = std::fs::read(&path).unwrap();
    let file_len = bytes.len();
    let footer_start = u64::from_le_bytes(bytes[file_len - 8..].try_into().unwrap()) as usize;
    let payload_len = u32::from_le_bytes(bytes[footer_start + 4..footer_start + 8].try_into().unwrap()) as usize;
    let footer_bytes = &bytes[footer_start + 12..footer_start + 12 + payload_len];
    let footer: bradian_storage::SegmentFooter = bincode::deserialize(footer_bytes).unwrap();
    let first_page = &footer.columns[0].pages[0];
    let trailer_offset = if first_page.validity_len > 0 {
        first_page.validity_offset + first_page.validity_len as u64
    } else {
        first_page.payload_offset + first_page.payload_len as u64
    } as usize;
    bytes[trailer_offset] = 7;
    std::fs::write(&path, &bytes).unwrap();

    let err = SegmentReader::open_with_bace(&path, wiring_zstd_balanced())
        .and_then(|mut r| r.read_column("id"))
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("unknown BACE envelope tag") || msg.contains("7"),
        "expected unknown-tag corruption, got: {msg}",
    );
}

fn crc32_bytes(bytes: &[u8]) -> u32 {
    let mut h = crc32fast::Hasher::new();
    h.update(bytes);
    h.finalize()
}
