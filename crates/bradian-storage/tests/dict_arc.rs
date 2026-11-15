//! ADR 0095 — verify that `ColumnBuffer::DictIndex32` holds
//! Arc-shared dict bytes. Two specific invariants:
//! 1. Clone shares the dict (strong_count increments).
//! 2. bincode round-trip still works (serde `rc` feature).

use std::sync::Arc;

use bradian_storage::{bitmap::Bitmap, ColumnBuffer};

#[test]
fn clone_shares_dict_bytes_via_arc() {
    let dict_offsets = Arc::new(vec![0u32, 1, 2, 3]);
    let dict_data = Arc::new(b"abc".to_vec());
    let mut v = Bitmap::with_capacity(3);
    v.push(true); v.push(true); v.push(true);
    let buf = ColumnBuffer::DictIndex32 {
        indices: vec![0, 1, 2],
        validity: v,
        dict_offsets: dict_offsets.clone(),
        dict_data: dict_data.clone(),
    };
    // Before clone: offsets Arc has 2 strong refs (local + buf).
    assert_eq!(Arc::strong_count(&dict_offsets), 2);
    let _c = buf.clone();
    // After clone: 3 refs — local, buf, _c.
    assert_eq!(Arc::strong_count(&dict_offsets), 3);
    assert_eq!(Arc::strong_count(&dict_data), 3);
}

#[test]
fn bincode_round_trip_preserves_values() {
    let mut v = Bitmap::with_capacity(4);
    v.push(true); v.push(true); v.push(true); v.push(true);
    let buf = ColumnBuffer::DictIndex32 {
        indices: vec![0, 1, 0, 2],
        validity: v,
        dict_offsets: Arc::new(vec![0, 1, 3, 6]),
        dict_data: Arc::new(b"abcdef".to_vec()),
    };
    let encoded = bincode::serialize(&buf).unwrap();
    let decoded: ColumnBuffer = bincode::deserialize(&encoded).unwrap();
    assert_eq!(buf, decoded);
}
