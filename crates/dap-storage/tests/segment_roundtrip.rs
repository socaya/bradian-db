//! End-to-end `.derc` v1 roundtrip tests: writer → file → reader → equality with the
//! originating builders.

use dap_core::{Column, LogicalType, Schema, Value};
use dap_storage::{ColumnBuffer, SegmentReader, SegmentWriter, SegmentWriterOptions};
use tempfile::tempdir;

fn sample_schema() -> Schema {
    Schema::new(vec![
        Column::new("ou", LogicalType::Uid).required(),
        Column::new("period", LogicalType::Period).required(),
        Column::new("de", LogicalType::Uid).required(),
        Column::new("value", LogicalType::F64),
        Column::new("count", LogicalType::I64),
        Column::new("flag", LogicalType::Bool),
    ])
    .unwrap()
}

fn sample_rows() -> Vec<Vec<Value>> {
    vec![
        vec![
            Value::Str("aaaaaaaaaaa".into()),
            Value::Str("202601".into()),
            Value::Str("deAAAAAAAAA".into()),
            Value::F64(42.5),
            Value::I64(100),
            Value::Bool(true),
        ],
        vec![
            Value::Str("bbbbbbbbbbb".into()),
            Value::Str("202601".into()),
            Value::Str("deAAAAAAAAA".into()),
            Value::F64(-1.25),
            Value::Null,
            Value::Bool(false),
        ],
        vec![
            Value::Str("aaaaaaaaaaa".into()),
            Value::Str("202602".into()),
            Value::Str("deBBBBBBBBB".into()),
            Value::F64(0.0),
            Value::I64(7),
            Value::Bool(true),
        ],
        vec![
            Value::Str("aaaaaaaaaaa".into()),
            Value::Str("202603".into()),
            Value::Str("deAAAAAAAAA".into()),
            Value::Null,
            Value::I64(-42),
            Value::Bool(true),
        ],
    ]
}

#[test]
fn segment_writer_reader_roundtrip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("test.derc");

    let schema = sample_schema();
    let rows = sample_rows();
    let mut writer = SegmentWriter::new(
        schema.clone(),
        SegmentWriterOptions {
            dataset_id: "aggDatasetX".into(),
            partition_id: "p1".into(),
            segment_id: "s1".into(),
            compress: true,
            dict_cardinality_threshold: 0.5,
            watermark_ms: 1_000,
            data_revision: 3,
            metadata_revision: None,
        },
    );
    for r in &rows {
        writer.append_row(r).unwrap();
    }
    let written = writer.finish_to_path(&path).unwrap();
    assert!(path.exists());
    assert_eq!(written.footer.row_count, rows.len() as u64);
    assert_eq!(written.footer.columns.len(), schema.len());

    // Read back.
    let mut reader = SegmentReader::open(&path).unwrap();
    assert_eq!(reader.footer().row_count, rows.len() as u64);
    assert_eq!(reader.footer().dataset_id, "aggDatasetX");

    // Scan + compare each column cell-by-cell.
    let mut buffers: Vec<ColumnBuffer> = Vec::new();
    for c in schema.columns.iter() {
        buffers.push(reader.read_column(&c.name).unwrap());
    }
    assert_eq!(buffers.len(), schema.len());

    for (col_idx, col) in schema.columns.iter().enumerate() {
        let buf = &buffers[col_idx];
        assert_eq!(buf.len(), rows.len(), "column {} len", col.name);
        for (r_idx, row) in rows.iter().enumerate() {
            let expected = &row[col_idx];
            let actual = buf.value_at(r_idx).unwrap();
            assert_values_equal(&actual, expected, &col.name, r_idx);
        }
    }
}

#[test]
fn reject_unsupported_major_version() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bad.derc");

    let schema = sample_schema();
    let w = SegmentWriter::new(schema, SegmentWriterOptions::default());
    w.finish_to_path(&path).unwrap();

    // Flip the major version byte.
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[8] = 0xFF;
    bytes[9] = 0xFF;
    std::fs::write(&path, &bytes).unwrap();

    let err = SegmentReader::open(&path).expect_err("version check");
    assert!(err.to_string().contains("major version"));
}

#[test]
fn detect_payload_corruption() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("corrupt.derc");

    let schema = Schema::new(vec![Column::new("v", LogicalType::I64)]).unwrap();
    let mut w = SegmentWriter::new(schema, SegmentWriterOptions::default());
    for i in 0..256 {
        w.append_row(&[Value::I64(i)]).unwrap();
    }
    w.finish_to_path(&path).unwrap();

    // Flip a byte inside the first page payload (just after the 64B header).
    let mut bytes = std::fs::read(&path).unwrap();
    let flip_at = 72;
    bytes[flip_at] ^= 0xFF;
    std::fs::write(&path, &bytes).unwrap();

    let mut reader = SegmentReader::open(&path).unwrap();
    let err = reader.read_column("v").expect_err("crc should trip");
    assert!(err.to_string().to_lowercase().contains("crc") || err.to_string().contains("corrupt"));
}

#[test]
fn detect_footer_corruption() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("footer_corrupt.derc");
    let schema = sample_schema();
    let mut w = SegmentWriter::new(schema, SegmentWriterOptions::default());
    for r in sample_rows() {
        w.append_row(&r).unwrap();
    }
    w.finish_to_path(&path).unwrap();

    // Flip one byte of the footer payload (just before the trailing u64 offset).
    let mut bytes = std::fs::read(&path).unwrap();
    let flip_at = bytes.len() - 16;
    bytes[flip_at] ^= 0x55;
    std::fs::write(&path, &bytes).unwrap();

    let err = SegmentReader::open(&path).expect_err("footer crc trips");
    assert!(err.to_string().to_lowercase().contains("crc") || err.to_string().contains("corrupt"));
}

#[test]
fn dict_encoding_selected_for_low_cardinality_utf8() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("dict.derc");

    let schema = Schema::new(vec![
        Column::new("category", LogicalType::Utf8).required(),
    ])
    .unwrap();
    let mut w = SegmentWriter::new(
        schema,
        SegmentWriterOptions {
            dataset_id: "d".into(),
            partition_id: "p".into(),
            segment_id: "s".into(),
            compress: true,
            dict_cardinality_threshold: 0.5,
            ..Default::default()
        },
    );
    // Only 3 distinct values across 1000 rows -> dict codec should win.
    for i in 0..1000 {
        let v = match i % 3 {
            0 => "alpha",
            1 => "beta",
            _ => "gamma",
        };
        w.append_row(&[Value::Str(v.into())]).unwrap();
    }
    w.finish_to_path(&path).unwrap();

    let reader = SegmentReader::open(&path).unwrap();
    let meta = reader.column("category").unwrap();
    assert_eq!(
        meta.pages[0].codec,
        dap_storage::CodecId::Dictionary,
        "expected dict codec for low-cardinality column"
    );
}

fn assert_values_equal(a: &Value, b: &Value, col: &str, row: usize) {
    use Value::*;
    let matches = match (a, b) {
        (Null, Null) => true,
        (I64(x), I64(y)) => x == y,
        (F64(x), F64(y)) => x.to_bits() == y.to_bits(),
        (Bool(x), Bool(y)) => x == y,
        (Str(x), Str(y)) => x == y,
        (Str(x), Period(y)) | (Period(y), Str(x)) => x == y,
        (Str(x), Uid(y)) | (Uid(y), Str(x)) => x == y.as_str(),
        _ => a == b,
    };
    assert!(matches, "col={col} row={row}: expected {b:?}, got {a:?}");
}
