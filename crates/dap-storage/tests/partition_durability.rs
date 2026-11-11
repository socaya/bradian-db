//! Integration: WAL + manifest + delta + flush lifecycle.
//!
//! Covers STO-002 / STO-004 / STO-005 / STO-006 (safe compaction) for the v1 single-writer
//! single-reader path.

use dap_core::{Column, LogicalType, Schema, Value};
use dap_storage::{Partition, PartitionOptions, PrimaryKey, WalReader};
use tempfile::tempdir;

fn schema() -> Schema {
    Schema::new(vec![
        Column::new("ou", LogicalType::Uid).required(),
        Column::new("period", LogicalType::Period).required(),
        Column::new("de", LogicalType::Uid).required(),
        Column::new("value", LogicalType::F64),
    ])
    .unwrap()
}

fn open_part(root: std::path::PathBuf) -> std::sync::Arc<Partition> {
    Partition::open(
        &root,
        PartitionOptions {
            dataset_id: "d1".into(),
            partition_id: "p1".into(),
            schema: schema(),
            primary_key_columns: vec!["ou".into(), "period".into(), "de".into()],
            flush_rows: 1024,
            flush_bytes: 1 << 24, // 16 MiB
        },
    )
    .unwrap()
}

#[test]
fn upsert_and_delete_visible_through_get() {
    let dir = tempdir().unwrap();
    let p = open_part(dir.path().to_path_buf());

    let ou = Value::Str("ouAAAAAAAAA".into());
    let per = Value::Str("202601".into());
    let de = Value::Str("deAAAAAAAAA".into());

    p.upsert_row(
        vec![ou.clone(), per.clone(), de.clone(), Value::F64(42.0)],
        1,
    )
    .unwrap();

    let key = PrimaryKey(vec![ou.clone(), per.clone(), de.clone()]);
    let row = p.get(&key).unwrap().unwrap();
    assert_eq!(row.0[3], Value::F64(42.0));

    p.delete_row(vec![ou, per, de], 2).unwrap();
    assert!(p.get(&key).unwrap().is_none());
}

#[test]
fn wal_replay_restores_delta_across_restart() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();

    {
        let p = open_part(root.clone());
        p.upsert_row(
            vec![
                Value::Str("ouAAAAAAAAA".into()),
                Value::Str("202601".into()),
                Value::Str("deAAAAAAAAA".into()),
                Value::F64(1.5),
            ],
            1,
        )
        .unwrap();
        p.upsert_row(
            vec![
                Value::Str("ouBBBBBBBBB".into()),
                Value::Str("202601".into()),
                Value::Str("deAAAAAAAAA".into()),
                Value::F64(2.5),
            ],
            1,
        )
        .unwrap();
        // No explicit flush — rows live only in WAL + delta.
    }

    // Reopen: WAL replay should rebuild the delta.
    let p = open_part(root);
    let key = PrimaryKey(vec![
        Value::Str("ouAAAAAAAAA".into()),
        Value::Str("202601".into()),
        Value::Str("deAAAAAAAAA".into()),
    ]);
    let row = p.get(&key).unwrap().unwrap();
    assert_eq!(row.0[3], Value::F64(1.5));
}

#[test]
fn flush_publishes_segment_and_truncates_wal() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let p = open_part(root.clone());

    for i in 0..5 {
        let ou = format!("o{:0>10}", i);
        p.upsert_row(
            vec![
                Value::Str(ou),
                Value::Str("202601".into()),
                Value::Str("deAAAAAAAAA".into()),
                Value::F64(i as f64),
            ],
            1,
        )
        .unwrap();
    }

    let seg = p.flush().unwrap().expect("flush produced a segment");
    assert!(seg.path.exists());
    assert_eq!(seg.row_count, 5);

    // After flush, WAL should be empty (we truncate after checkpoint).
    let wal_path = p.dir().join("wal.log");
    let recs = WalReader::open(&wal_path).replay().unwrap();
    assert!(recs.is_empty(), "WAL truncated after checkpoint");

    // Get() should read the segment (delta is empty).
    let key = PrimaryKey(vec![
        Value::Str("o0000000000".into()),
        Value::Str("202601".into()),
        Value::Str("deAAAAAAAAA".into()),
    ]);
    let row = p.get(&key).unwrap().unwrap();
    assert_eq!(row.0[3], Value::F64(0.0));
}

#[test]
fn version_fence_drops_stale_replay() {
    let dir = tempdir().unwrap();
    let p = open_part(dir.path().to_path_buf());

    let key = PrimaryKey(vec![
        Value::Str("ouAAAAAAAAA".into()),
        Value::Str("202601".into()),
        Value::Str("deAAAAAAAAA".into()),
    ]);
    let full = |val: f64| {
        vec![
            Value::Str("ouAAAAAAAAA".into()),
            Value::Str("202601".into()),
            Value::Str("deAAAAAAAAA".into()),
            Value::F64(val),
        ]
    };

    p.upsert_row(full(10.0), 5).unwrap();
    // Replay a stale version — must not overwrite the current state.
    p.upsert_row(full(99.0), 3).unwrap();
    let row = p.get(&key).unwrap().unwrap();
    assert_eq!(row.0[3], Value::F64(10.0));

    // Newer version supersedes.
    p.upsert_row(full(20.0), 7).unwrap();
    let row = p.get(&key).unwrap().unwrap();
    assert_eq!(row.0[3], Value::F64(20.0));
}

#[test]
fn compact_all_segments_merges_and_deduplicates() {
    let dir = tempdir().unwrap();
    let p = open_part(dir.path().to_path_buf());

    // Flush three segments; the second flush updates the same key (ouAAAAAAAAA/202601/
    // deAAAAAAAAA) with a higher data revision so compaction must keep only the latest.
    p.upsert_row(
        vec![
            Value::Str("ouAAAAAAAAA".into()),
            Value::Str("202601".into()),
            Value::Str("deAAAAAAAAA".into()),
            Value::F64(10.0),
        ],
        1,
    )
    .unwrap();
    p.flush().unwrap();

    p.upsert_row(
        vec![
            Value::Str("ouAAAAAAAAA".into()),
            Value::Str("202601".into()),
            Value::Str("deAAAAAAAAA".into()),
            Value::F64(999.0),
        ],
        2,
    )
    .unwrap();
    p.flush().unwrap();

    p.upsert_row(
        vec![
            Value::Str("ouBBBBBBBBB".into()),
            Value::Str("202601".into()),
            Value::Str("deAAAAAAAAA".into()),
            Value::F64(20.0),
        ],
        1,
    )
    .unwrap();
    p.flush().unwrap();

    assert_eq!(p.visible_segments().len(), 3);

    let merged = p
        .compact_all_segments()
        .unwrap()
        .expect("compaction produced a segment");
    assert_eq!(p.visible_segments().len(), 1);
    assert_eq!(merged.row_count, 2, "deduped by primary key");

    // Verify the newer value survived.
    let key = PrimaryKey(vec![
        Value::Str("ouAAAAAAAAA".into()),
        Value::Str("202601".into()),
        Value::Str("deAAAAAAAAA".into()),
    ]);
    let row = p.get(&key).unwrap().unwrap();
    assert_eq!(row.0[3], Value::F64(999.0));
}

#[test]
fn snapshot_prevents_compaction_from_breaking_reader() {
    let dir = tempdir().unwrap();
    let p = open_part(dir.path().to_path_buf());

    for i in 0..3 {
        p.upsert_row(
            vec![
                Value::Str(format!("ou{:0>9}", i)),
                Value::Str("202601".into()),
                Value::Str("deAAAAAAAAA".into()),
                Value::F64(i as f64),
            ],
            1,
        )
        .unwrap();
    }

    let s0 = p.snapshot();
    assert_eq!(s0.generation(), 0);
    let _seg = p.flush().unwrap().unwrap();
    let s1 = p.snapshot();
    assert_eq!(s1.generation(), 1);
}

#[test]
fn compaction_defers_segment_deletion_while_snapshot_is_live() {
    use std::path::PathBuf;

    let dir = tempdir().unwrap();
    let p = open_part(dir.path().to_path_buf());

    // Two separate flushes → two segments.
    p.upsert_row(
        vec![
            Value::Str("ouAAAAAAAAA".into()),
            Value::Str("202601".into()),
            Value::Str("deAAAAAAAAA".into()),
            Value::F64(1.0),
        ],
        1,
    )
    .unwrap();
    p.flush().unwrap();
    p.upsert_row(
        vec![
            Value::Str("ouBBBBBBBBB".into()),
            Value::Str("202601".into()),
            Value::Str("deAAAAAAAAA".into()),
            Value::F64(2.0),
        ],
        1,
    )
    .unwrap();
    p.flush().unwrap();

    let pre_paths: Vec<PathBuf> =
        p.visible_segments().iter().map(|s| s.path.clone()).collect();
    assert_eq!(pre_paths.len(), 2);

    // Hold a snapshot pinned to the pre-compaction generation.
    let snap = p.snapshot();
    let pinned_gen = snap.generation();

    p.compact_all_segments().unwrap();

    // Both pre-compaction segment files must still be on disk — the snapshot still
    // references them. The new merged segment is a third file, separate from the two.
    let compacted = p.visible_segments();
    assert_eq!(compacted.len(), 1);
    for p0 in &pre_paths {
        assert!(
            p0.exists(),
            "retired segment {p0:?} must stay on disk while snapshot gen {pinned_gen} is live"
        );
    }
    assert_eq!(p.pending_deletion_count(), 2);

    // Drop the snapshot and request an explicit cleanup. Both retired files are reaped.
    drop(snap);
    let reaped = p.cleanup_pending_deletions();
    assert_eq!(reaped, 2);
    assert_eq!(p.pending_deletion_count(), 0);
    for p0 in &pre_paths {
        assert!(
            !p0.exists(),
            "retired segment {p0:?} must be deleted once no snapshot references it"
        );
    }
    // The merged segment is untouched.
    assert!(compacted[0].path.exists());
}

#[test]
fn next_flush_sweeps_pending_deletions() {
    use std::path::PathBuf;

    let dir = tempdir().unwrap();
    let p = open_part(dir.path().to_path_buf());

    for (i, ou) in ["ouAAAAAAAAA", "ouBBBBBBBBB"].iter().enumerate() {
        p.upsert_row(
            vec![
                Value::Str((*ou).into()),
                Value::Str("202601".into()),
                Value::Str("deAAAAAAAAA".into()),
                Value::F64(i as f64),
            ],
            1,
        )
        .unwrap();
        p.flush().unwrap();
    }

    let retired: Vec<PathBuf> =
        p.visible_segments().iter().map(|s| s.path.clone()).collect();

    let snap = p.snapshot();
    p.compact_all_segments().unwrap();
    assert_eq!(p.pending_deletion_count(), 2);
    drop(snap);

    // A subsequent write + flush sweeps pending deletions as a side effect.
    p.upsert_row(
        vec![
            Value::Str("ouCCCCCCCCC".into()),
            Value::Str("202601".into()),
            Value::Str("deAAAAAAAAA".into()),
            Value::F64(42.0),
        ],
        1,
    )
    .unwrap();
    p.flush().unwrap();

    assert_eq!(p.pending_deletion_count(), 0);
    for r in &retired {
        assert!(!r.exists(), "{r:?} swept by next flush");
    }
}
