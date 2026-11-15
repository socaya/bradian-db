//! ADR 0084 — dict-union concat for differing-dict `DictIndex32`
//! pages. Reader-only behavior; no on-disk format change. Tests
//! drive `ColumnBuffer::concat_dict_pages` directly because no
//! v1 writer produces differing-dict pages today (see ADR
//! 0084 §10 honest forward-compat note).

use bradian_storage::bitmap::Bitmap;
use bradian_storage::ColumnBuffer;

fn dict_page(indices: Vec<u32>, valid: Vec<bool>, dict: &[&str]) -> ColumnBuffer {
    let mut validity = Bitmap::with_capacity(indices.len());
    for v in &valid { validity.push(*v); }
    let mut offsets: Vec<u32> = vec![0];
    let mut data: Vec<u8> = Vec::new();
    for s in dict {
        data.extend_from_slice(s.as_bytes());
        offsets.push(data.len() as u32);
    }
    ColumnBuffer::DictIndex32 {
        indices, validity,
        dict_offsets: std::sync::Arc::new(offsets),
        dict_data: std::sync::Arc::new(data),
    }
}

fn str_at(col: &ColumnBuffer, i: usize) -> Option<String> {
    match col.value_at(i).unwrap() {
        bradian_core::Value::Str(s) => Some(s),
        bradian_core::Value::Null => None,
        other => panic!("unexpected {other:?}"),
    }
}

// ---------- §3.5 item 2 — fast path (identical dicts) ----------

#[test]
fn item_2_identical_dicts_fast_path_preserves_dict_bytes() {
    let p1 = dict_page(vec![0, 1], vec![true, true], &["a", "b"]);
    let p2 = dict_page(vec![1, 0], vec![true, true], &["a", "b"]);
    let out = ColumnBuffer::concat_dict_pages(vec![p1, p2]).unwrap();
    let ColumnBuffer::DictIndex32 { indices, dict_offsets, dict_data, .. } = &out
        else { panic!("expected DictIndex32"); };
    assert_eq!(indices, &vec![0, 1, 1, 0]);
    assert_eq!(dict_offsets.as_slice(), &[0, 1, 2]);
    assert_eq!(dict_data.as_slice(), b"ab");
    assert_eq!(str_at(&out, 0), Some("a".into()));
    assert_eq!(str_at(&out, 3), Some("a".into()));
}

// ---------- §3.5 item 3 — non-overlapping dicts ----------

#[test]
fn item_3_non_overlapping_dicts_produce_unioned_output() {
    let p1 = dict_page(vec![0, 1], vec![true, true], &["a", "b"]);
    let p2 = dict_page(vec![0, 1], vec![true, true], &["c", "d"]);
    let out = ColumnBuffer::concat_dict_pages(vec![p1, p2]).unwrap();
    let ColumnBuffer::DictIndex32 { indices, dict_offsets, dict_data, .. } = &out
        else { panic!("expected DictIndex32"); };
    // Union order: a, b (from page 1), then c, d (from page 2).
    assert_eq!(dict_data.as_slice(), b"abcd");
    assert_eq!(dict_offsets.as_slice(), &[0, 1, 2, 3, 4]);
    // Page 2's indices 0,1 remap to union indices 2,3.
    assert_eq!(indices, &vec![0, 1, 2, 3]);
    assert_eq!(str_at(&out, 0), Some("a".into()));
    assert_eq!(str_at(&out, 1), Some("b".into()));
    assert_eq!(str_at(&out, 2), Some("c".into()));
    assert_eq!(str_at(&out, 3), Some("d".into()));
}

// ---------- §3.5 item 4 — partially overlapping dicts ----------

#[test]
fn item_4_partial_overlap_dedups_shared_entries() {
    let p1 = dict_page(vec![0, 1], vec![true, true], &["a", "b"]);
    let p2 = dict_page(vec![0, 1], vec![true, true], &["b", "c"]);
    let out = ColumnBuffer::concat_dict_pages(vec![p1, p2]).unwrap();
    let ColumnBuffer::DictIndex32 { indices, dict_offsets, dict_data, .. } = &out
        else { panic!("expected DictIndex32"); };
    // Union: a, b (page 1), then c (page 2's new entry; b dedup'd).
    assert_eq!(dict_data.as_slice(), b"abc");
    assert_eq!(dict_offsets.as_slice(), &[0, 1, 2, 3]);
    // Page 2 row 0 ("b") → union index 1, row 1 ("c") → union index 2.
    assert_eq!(indices, &vec![0, 1, 1, 2]);
    assert_eq!(str_at(&out, 0), Some("a".into()));
    assert_eq!(str_at(&out, 1), Some("b".into()));
    assert_eq!(str_at(&out, 2), Some("b".into()));
    assert_eq!(str_at(&out, 3), Some("c".into()));
}

// ---------- §3.5 item 5 — null sentinel passes through ----------

#[test]
fn item_5_null_sentinel_passthrough() {
    let p1 = dict_page(vec![0, u32::MAX, 1], vec![true, false, true], &["a", "b"]);
    let p2 = dict_page(vec![u32::MAX, 0], vec![false, true], &["c"]);
    let out = ColumnBuffer::concat_dict_pages(vec![p1, p2]).unwrap();
    let ColumnBuffer::DictIndex32 { indices, dict_offsets, dict_data, .. } = &out
        else { panic!("expected DictIndex32"); };
    assert_eq!(dict_data.as_slice(), b"abc");
    assert_eq!(dict_offsets.as_slice(), &[0, 1, 2, 3]);
    // Page 1: a, null, b. Page 2: null, c (page 2's "c" → union idx 2).
    assert_eq!(indices, &vec![0, u32::MAX, 1, u32::MAX, 2]);
    assert_eq!(str_at(&out, 0), Some("a".into()));
    assert_eq!(str_at(&out, 1), None);
    assert_eq!(str_at(&out, 2), Some("b".into()));
    assert_eq!(str_at(&out, 3), None);
    assert_eq!(str_at(&out, 4), Some("c".into()));
}

// ---------- §3.5 item 6 — determinism ----------

#[test]
fn item_6_union_order_is_stable_across_runs() {
    let build = || {
        let p1 = dict_page(vec![1, 0, 1], vec![true, true, true], &["x", "y"]);
        let p2 = dict_page(vec![0, 1], vec![true, true], &["y", "z"]);
        let p3 = dict_page(vec![0, 1], vec![true, true], &["w", "x"]);
        ColumnBuffer::concat_dict_pages(vec![p1, p2, p3]).unwrap()
    };
    let a = build();
    let b = build();
    let (ColumnBuffer::DictIndex32 { dict_data: ad, dict_offsets: ao, indices: ai, .. },
         ColumnBuffer::DictIndex32 { dict_data: bd, dict_offsets: bo, indices: bi, .. })
        = (&a, &b) else { panic!(); };
    assert_eq!(ad, bd);
    assert_eq!(ao, bo);
    assert_eq!(ai, bi);
    // Expected order: x, y (page 1), z (page 2 adds z), w (page 3 adds w).
    assert_eq!(ad.as_slice(), b"xyzw");
}

// ---------- §3.5 item 7 — empty-dict / zero-row page ----------

#[test]
fn item_7_empty_dict_page_tolerated() {
    // Edge: a page with empty dict and zero rows contributes
    // nothing to the union. The other page's output is preserved.
    let p_empty = ColumnBuffer::DictIndex32 {
        indices: vec![],
        validity: Bitmap::default(),
        dict_offsets: std::sync::Arc::new(vec![0]),
        dict_data: std::sync::Arc::new(Vec::new()),
    };
    let p_data = dict_page(vec![0, 1], vec![true, true], &["a", "b"]);
    let out = ColumnBuffer::concat_dict_pages(vec![p_empty, p_data]).unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(str_at(&out, 0), Some("a".into()));
    assert_eq!(str_at(&out, 1), Some("b".into()));
}

// ---------- §3.5 item 8 — single-page short-circuit ----------

#[test]
fn item_8_single_page_input_is_returned_verbatim() {
    let p = dict_page(vec![0, 1, 0], vec![true, true, true], &["a", "b"]);
    let out = ColumnBuffer::concat_dict_pages(vec![p.clone()]).unwrap();
    assert_eq!(out, p);
}

// ---------- §3.5 item 9 — mixed physical-type input errors ----------

#[test]
fn item_9_mixed_physical_type_input_is_rejected() {
    let d = dict_page(vec![0], vec![true], &["a"]);
    let u = ColumnBuffer::Utf8 {
        offsets: vec![0, 1], data: b"z".to_vec(),
        validity: {
            let mut v = Bitmap::with_capacity(1); v.push(true); v
        },
    };
    let err = ColumnBuffer::concat_dict_pages(vec![d, u]).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("not DictIndex32"), "got: {msg}");
}

// ---------- §3.5 item 10 — honest coverage note ----------

#[test]
fn item_10_three_page_union_round_trips() {
    // No v1 writer produces this shape, but we demonstrate the
    // reader contract that a future per-page-sub-dict writer
    // ADR would depend on.
    let p1 = dict_page(vec![0, 1, 0], vec![true, true, true], &["alpha", "beta"]);
    let p2 = dict_page(vec![1, 0], vec![true, true], &["gamma", "alpha"]);
    let p3 = dict_page(vec![0, u32::MAX], vec![true, false], &["delta"]);

    let out = ColumnBuffer::concat_dict_pages(vec![p1, p2, p3]).unwrap();
    assert_eq!(out.len(), 7);
    assert!(matches!(out, ColumnBuffer::DictIndex32 { .. }),
        "output must preserve DictIndex32 shape, not promote to Utf8");
    assert_eq!(str_at(&out, 0), Some("alpha".into()));
    assert_eq!(str_at(&out, 1), Some("beta".into()));
    assert_eq!(str_at(&out, 2), Some("alpha".into()));
    assert_eq!(str_at(&out, 3), Some("alpha".into()));   // page 2 row 0: idx 1 ("alpha")
    assert_eq!(str_at(&out, 4), Some("gamma".into()));   // page 2 row 1: idx 0 ("gamma")
    assert_eq!(str_at(&out, 5), Some("delta".into()));
    assert_eq!(str_at(&out, 6), None);
}

// ---------- §3.5 item 1 — existing tests still pass (compile check) ----------

#[test]
fn item_1_existing_adr_0082_fast_path_untouched() {
    // A simple identical-dict two-page concat must still
    // produce DictIndex32 output. This duplicates part of
    // ADR 0082's item_5 but proves the delegation preserves
    // the fast path.
    let p1 = dict_page(vec![0, 1], vec![true, true], &["x", "y"]);
    let p2 = dict_page(vec![1, 0], vec![true, true], &["x", "y"]);
    let out = ColumnBuffer::concat_dict_pages(vec![p1, p2]).unwrap();
    let ColumnBuffer::DictIndex32 { dict_data, .. } = &out
        else { panic!("expected DictIndex32"); };
    assert_eq!(dict_data.as_slice(), b"xy");
}
