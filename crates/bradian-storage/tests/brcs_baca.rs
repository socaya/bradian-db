//! ADR 0080 — BACA carrier integration for `.brcs`.
//!
//! Covers the §3.5 checklist: v1.3 writer emission, round-trip
//! under matching key, cross-version reject paths (v1.0/v1.1/
//! v1.2 reject v1.3), wrong-key fail-closed, tamper fail-closed,
//! dict-path ignores auth, empty-auth produces v1.1 output.

use std::collections::HashMap;
use std::sync::Arc;

use bradian_relational::bace::codec::raw::RAW_ID;
use bradian_relational::bace::codec::zstd::ZSTD_ID;
use bradian_relational::bace::dictionary::train_zstd_dictionary;
use bradian_relational::bace::policy::{
    CompressionProfile, InMemoryPolicyService, PlatformBaseline,
};
use bradian_relational::bace::registry::BuiltInRegistry;
use bradian_relational::bace::{BaceAuthentication, BaceDictionary};
use bradian_relational::row_store::BaceWiring;
use bradian_core::{Column, LogicalType, Schema, Value};
use bradian_storage::{ColumnBuffer, SegmentReader, SegmentWriter, SegmentWriterOptions};
use tempfile::tempdir;

const KEY_PRIMARY: [u8; 32] = [7; 32];
const KEY_SECONDARY: [u8; 32] = [11; 32];

fn wiring_with_auth(key: [u8; 32], label: &str) -> BaceWiring {
    let baseline = PlatformBaseline {
        profile: CompressionProfile::Balanced,
        allowed_codecs: vec![ZSTD_ID, RAW_ID],
        ..PlatformBaseline::shipped()
    };
    BaceWiring {
        policy_service: Arc::new(InMemoryPolicyService::new(baseline, BuiltInRegistry::new())),
        registry: Arc::new(BuiltInRegistry::new()),
        authentication: Some(BaceAuthentication::new(key, label)),
        strict_authenticated: false,
    }
}

fn wiring_no_auth() -> BaceWiring {
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

fn schema() -> Schema {
    Schema::new(vec![
        Column::new("id", LogicalType::I64).required(),
        Column::new("tag", LogicalType::Utf8).required(),
    ]).unwrap()
}

fn rows() -> Vec<Vec<Value>> {
    (0..32).map(|i| vec![
        Value::I64(i),
        Value::Str(format!("tag-{:04}", i % 4)),
    ]).collect()
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

fn write(path: &std::path::Path, wiring: BaceWiring) {
    let mut w = SegmentWriter::new_with_bace(schema(), opts(), wiring);
    for r in &rows() { w.append_row(r).unwrap(); }
    w.finish_to_path(path).unwrap();
}

fn write_with_dict(path: &std::path::Path, wiring: BaceWiring, dicts: HashMap<String, BaceDictionary>) {
    let mut w = SegmentWriter::new_with_bace_and_dictionaries(schema(), opts(), wiring, dicts);
    for r in &rows() { w.append_row(r).unwrap(); }
    w.finish_to_path(path).unwrap();
}

fn assert_round_trip(reader: &mut SegmentReader) {
    let got = reader.read_column("id").unwrap();
    let ColumnBuffer::I64 { data, .. } = got else { panic!("expected I64"); };
    assert_eq!(data.len(), 32);
    for (i, v) in data.iter().enumerate() {
        assert_eq!(*v, i as i64);
    }
}

// ---------- §3.5 item 1: v1.3 writer emission ----------

#[test]
fn item_1_v1_3_writer_emits_minor_3_and_authenticated_flag() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("v13.brcs");
    write(&path, wiring_with_auth(KEY_PRIMARY, "primary"));

    let bytes = std::fs::read(&path).unwrap();
    let minor = u16::from_le_bytes([bytes[10], bytes[11]]);
    assert_eq!(minor, 3, "v1.3 writer must emit minor=3");
    let required = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
    assert_eq!(required & (1 << 5), 1 << 5, "BACE_PAGE_ENVELOPE must be set");
    assert_eq!(required & (1 << 7), 1 << 7, "BACE_PAGE_AUTHENTICATED must be set");
    assert_eq!(required & (1 << 6), 0, "BACE_PAGE_DICTIONARY must NOT be set (no dicts)");
}

// ---------- §3.5 item 2: round-trip with matching key ----------

#[test]
fn item_2_round_trip_under_matching_key() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("rt.brcs");
    write(&path, wiring_with_auth(KEY_PRIMARY, "primary"));
    let mut r = SegmentReader::open_with_bace(
        &path, wiring_with_auth(KEY_PRIMARY, "primary"),
    ).unwrap();
    assert_round_trip(&mut r);
}

// ---------- §3.5 item 3: v1.0/v1.1/v1.2 reject v1.3 ----------

#[test]
fn item_3_legacy_reader_rejects_v1_3_segment() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("v13.brcs");
    write(&path, wiring_with_auth(KEY_PRIMARY, "primary"));
    let err = SegmentReader::open(&path).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("not supported") || msg.contains("feature bits"),
        "expected unsupported-feature error, got: {msg}",
    );
}

// ---------- §3.5 item 4: wiring without auth on BACA-trailer segment ----------

#[test]
fn item_4_reader_without_authentication_fails_unsupported() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("v13.brcs");
    write(&path, wiring_with_auth(KEY_PRIMARY, "primary"));

    let err = SegmentReader::open_with_bace(&path, wiring_no_auth())
        .and_then(|mut r| r.read_column("id"))
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("BACA") && msg.contains("authentication"),
        "expected missing-authentication error, got: {msg}",
    );
}

// ---------- §3.5 item 5: wrong key ----------

#[test]
fn item_5_reader_with_wrong_key_fails_corruption() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("v13.brcs");
    write(&path, wiring_with_auth(KEY_PRIMARY, "primary"));

    let err = SegmentReader::open_with_bace(
        &path, wiring_with_auth(KEY_SECONDARY, "secondary"),
    )
    .and_then(|mut r| r.read_column("id"))
    .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("MAC") || msg.contains("wrong key") || msg.contains("tampered"),
        "expected MAC-mismatch error, got: {msg}",
    );
}

// ---------- §3.5 item 6: tamper inside envelope ----------

#[test]
fn item_6_tampered_envelope_bytes_fail_closed() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("tamper.brcs");
    write(&path, wiring_with_auth(KEY_PRIMARY, "primary"));
    let mut bytes = std::fs::read(&path).unwrap();
    // Flip a byte inside the first page body (right after the
    // 64-byte segment header). The MAC should detect this.
    bytes[64 + 40] ^= 0xFF;
    std::fs::write(&path, &bytes).unwrap();

    let err = SegmentReader::open_with_bace(
        &path, wiring_with_auth(KEY_PRIMARY, "primary"),
    )
    .and_then(|mut r| r.read_column("id"))
    .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("MAC") || msg.contains("CRC") || msg.contains("envelope")
            || msg.contains("tampered") || msg.contains("corruption"),
        "expected tamper-detected error, got: {msg}",
    );
}

// ---------- §3.5 item 7: trailer-flip BACE→BACA mismatch ----------

#[test]
fn item_7_trailer_flip_from_bace_to_baca_fails_magic_mismatch() {
    // Write a BACE segment (no auth); flip a page's trailer
    // byte from 1 to 2, then try to read with wiring that has
    // authentication. The reader will try to parse a BACA
    // envelope but the on-disk bytes are a BACE envelope with
    // "BACE" magic, firing a magic-mismatch error.
    let dir = tempdir().unwrap();
    let path = dir.path().join("flip.brcs");
    write(&path, wiring_no_auth());

    // Locate and flip a trailer byte. Use the same approach as
    // the ADR 0076 item_9 test.
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
    assert_eq!(bytes[trailer_offset], 1, "expected BACE trailer");
    bytes[trailer_offset] = 2;
    std::fs::write(&path, &bytes).unwrap();

    // Also need to flip the feature flag so the v1.3 reader
    // doesn't reject at the header level — simulating a crafted
    // v1.3-looking segment whose actual envelope bytes are BACE.
    // The u16 minor is at offset 10 and feature flags at offset 12.
    // Set BACE_PAGE_AUTHENTICATED bit (1 << 7).
    let mut bytes = std::fs::read(&path).unwrap();
    let flags = u64::from_le_bytes(bytes[12..20].try_into().unwrap()) | (1 << 7);
    bytes[12..20].copy_from_slice(&flags.to_le_bytes());
    bytes[10..12].copy_from_slice(&3u16.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();

    let err = SegmentReader::open_with_bace(
        &path, wiring_with_auth(KEY_PRIMARY, "primary"),
    )
    .and_then(|mut r| r.read_column("id"))
    .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("MAC") || msg.contains("magic") || msg.contains("BACA")
            || msg.contains("envelope") || msg.contains("tampered"),
        "expected envelope/magic/MAC error, got: {msg}",
    );
}

// ---------- §3.5 item 8: empty authentication produces v1.1 output ----------

#[test]
fn item_8_wiring_without_authentication_produces_v1_1_output() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("v11.brcs");
    write(&path, wiring_no_auth());

    let bytes = std::fs::read(&path).unwrap();
    let minor = u16::from_le_bytes([bytes[10], bytes[11]]);
    assert_eq!(minor, 1, "wiring without auth must emit v1.1 minor=1");
    let required = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
    assert_eq!(required & (1 << 7), 0,
        "wiring without auth must NOT set BACE_PAGE_AUTHENTICATED");
    assert_eq!(required & (1 << 5), 1 << 5,
        "wiring without auth still sets BACE_PAGE_ENVELOPE");
}

// ---------- §3.5 item 9: dict path ignores authentication ----------

#[test]
fn item_9_dict_path_ignores_authentication_emits_trailer_3() {
    // Even when wiring has authentication, a dict-equipped column
    // writes trailer 3 (ADR 0079), not trailer 2 (ADR 0080). This
    // is the §2 non-goal check.
    let dir = tempdir().unwrap();
    let path = dir.path().join("dict_auth.brcs");

    let samples_owned: Vec<Vec<u8>> = (0..64).map(|i| {
        format!("tag-{:04}", i % 4).into_bytes()
    }).collect();
    let refs: Vec<&[u8]> = samples_owned.iter().map(|v| v.as_slice()).collect();
    let dict = train_zstd_dictionary(&refs, 1024, "tag").unwrap();

    let mut dicts = HashMap::new();
    dicts.insert("tag".to_string(), dict);
    write_with_dict(&path, wiring_with_auth(KEY_PRIMARY, "primary"), dicts);

    let bytes = std::fs::read(&path).unwrap();
    let minor = u16::from_le_bytes([bytes[10], bytes[11]]);
    assert_eq!(minor, 2, "dict-path with auth emits v1.2 (dict), NOT v1.3 (auth)");
    let required = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
    assert_eq!(required & (1 << 6), 1 << 6, "BACE_PAGE_DICTIONARY must be set");
    assert_eq!(required & (1 << 7), 0,
        "BACE_PAGE_AUTHENTICATED must NOT be set when dicts present (§2 non-goal)");

    // Round-trip still works via the wiring (dict decode path
    // ignores the auth).
    let mut r = SegmentReader::open_with_bace(
        &path, wiring_with_auth(KEY_PRIMARY, "primary"),
    ).unwrap();
    assert_round_trip(&mut r);
}

// ---------- §3.5 item 10: BaceAuthentication label in error chain ----------

#[test]
fn item_10_wrong_key_error_names_the_wiring_label() {
    // ADR 0075 §3.4 — the BACA MAC-mismatch error carries the
    // reader's auth label. This test confirms that information
    // survives through the bradian-storage error chain.
    let dir = tempdir().unwrap();
    let path = dir.path().join("rt.brcs");
    write(&path, wiring_with_auth(KEY_PRIMARY, "primary"));
    let err = SegmentReader::open_with_bace(
        &path, wiring_with_auth(KEY_SECONDARY, "ops-reader"),
    )
    .and_then(|mut r| r.read_column("id"))
    .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("ops-reader"),
        "expected reader's auth label in error chain, got: {msg}",
    );
}
