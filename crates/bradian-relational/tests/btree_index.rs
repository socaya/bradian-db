//! Physical B+tree pages and their integration with durable row checkpoints.

use std::fs;
use bradian_relational::btree::{DiskBTree, IndexError, RowPointer};
use bradian_relational::row_store::{RowStore, StoreError};
use tempfile::tempdir;

#[test]
fn empty_and_single_leaf_indexes_are_queryable() {
    let dir = tempdir().unwrap();
    let empty = dir.path().join("empty.bri");
    DiskBTree::build(&empty, 4, &[]).unwrap();
    let mut index = DiskBTree::open(&empty, 4).unwrap();
    assert!(index.is_empty());
    assert_eq!(index.get(b"k").unwrap(), None);
    assert!(index.range(b"a", b"z").unwrap().is_empty());

    let path = dir.path().join("one.bri");
    DiskBTree::build(&path, 7, &[
        (b"alpha".to_vec(), RowPointer { page_id: 1, offset: 0 }),
        (b"beta".to_vec(), RowPointer { page_id: 1, offset: 20 }),
        (b"zulu".to_vec(), RowPointer { page_id: 4, offset: 30 }),
    ]).unwrap();
    let mut index = DiskBTree::open(&path, 7).unwrap();
    assert_eq!(index.len(), 3);
    assert_eq!(index.get(b"beta").unwrap(), Some(RowPointer { page_id: 1, offset: 20 }));
    assert_eq!(index.get(b"missing").unwrap(), None);
    assert_eq!(index.range(b"beta", b"zulu").unwrap().iter().map(|(k,_)|k.clone()).collect::<Vec<_>>(),
        vec![b"beta".to_vec()]);
}

#[test]
fn multi_level_btree_resolves_all_keys_and_cross_leaf_ranges() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("deep.bri");
    let data = (0..4700_u64).map(|i| (
        format!("bradian-index-{i:06}-{}", "x".repeat(82)).into_bytes(),
        RowPointer { page_id: 1 + (i / 11), offset: (i % 11 * 20) as u16 }
    )).collect::<Vec<_>>();
    DiskBTree::build(&path, 71, &data).unwrap();
    let mut index = DiskBTree::open(&path, 71).unwrap();
    assert_eq!(index.len(), 4700);
    for (key, pointer) in &data {
        assert_eq!(index.get(key).unwrap(), Some(*pointer), "key not found");
    }
    let part = index.range(&data[911].0, &data[1037].0).unwrap();
    assert_eq!(part.len(), 126);
    assert_eq!(part.first().unwrap().0, data[911].0);
    assert_eq!(part.last().unwrap().0, data[1036].0);
    assert_eq!(index.get(b"bradian-index-999999").unwrap(), None);
}

#[test]
fn index_builder_rejects_duplicates_out_of_order_and_invalid_pointers() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bad.bri");
    let ptr = RowPointer { page_id: 1, offset: 0 };
    assert!(matches!(DiskBTree::build(&path, 1, &[
        (b"dup".to_vec(), ptr), (b"dup".to_vec(), ptr),
    ]), Err(IndexError::Invalid(_))));
    assert!(matches!(DiskBTree::build(&path, 1, &[
        (b"z".to_vec(), ptr), (b"a".to_vec(), ptr),
    ]), Err(IndexError::Invalid(_))));
    assert!(matches!(DiskBTree::build(&path, 1, &[
        (b"a".to_vec(), RowPointer { page_id: 0, offset: 1 }),
    ]), Err(IndexError::Invalid(_))));
    assert!(!path.exists());
}

#[test]
fn index_page_corruption_and_wrong_lsn_fail_closed() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("good.bri");
    DiskBTree::build(&path, 5, &[
        (b"a".to_vec(), RowPointer { page_id: 1, offset: 0 }),
    ]).unwrap();
    assert!(matches!(DiskBTree::open(&path, 6), Err(IndexError::Cache(_))));
    let mut bytes = fs::read(&path).unwrap();
    bytes[4096 + 34] ^= 0x41;
    fs::write(&path, bytes).unwrap();
    let mut index = DiskBTree::open(&path, 5).unwrap();
    assert!(matches!(index.get(b"a"), Err(IndexError::Cache(_))));
}

#[test]
fn row_store_checkpoint_publishes_and_reads_btree_index() {
    let dir = tempdir().unwrap();
    let store = RowStore::open(dir.path()).unwrap();
    for i in 0..700_u32 {
        store.put(format!("inv-{i:04}").as_bytes(), format!("amount-{i}").as_bytes()).unwrap();
    }
    assert_eq!(store.checkpoint().unwrap(), 700);
    let index_path = dir.path().join("index-700.bri");
    assert!(index_path.exists());
    assert_eq!(store.indexed_get(b"inv-0000").unwrap(), Some(b"amount-0".to_vec()));
    assert_eq!(store.indexed_get(b"inv-0699").unwrap(), Some(b"amount-699".to_vec()));
    assert_eq!(store.indexed_get(b"unknown").unwrap(), None);
    let subset = store.indexed_range(b"inv-0340", b"inv-0350").unwrap();
    assert_eq!(subset.len(), 10);
    assert_eq!(subset[0], (b"inv-0340".to_vec(), b"amount-340".to_vec()));
    assert_eq!(subset[9], (b"inv-0349".to_vec(), b"amount-349".to_vec()));
    drop(store);

    let reopened = RowStore::open(dir.path()).unwrap();
    assert_eq!(reopened.indexed_get(b"inv-0349").unwrap(), Some(b"amount-349".to_vec()));
    assert_eq!(reopened.indexed_range(b"inv-0000", b"inv-0700").unwrap().len(), 700);
}

#[test]
fn new_wal_mutations_do_not_leak_stale_index_results() {
    let dir = tempdir().unwrap();
    let store = RowStore::open(dir.path()).unwrap();
    store.put(b"alpha", b"old").unwrap();
    store.put(b"beta", b"keep").unwrap();
    store.checkpoint().unwrap();
    assert_eq!(store.indexed_get(b"alpha").unwrap(), Some(b"old".to_vec()));
    store.put(b"alpha", b"new").unwrap();
    store.delete(b"beta").unwrap();
    store.put(b"gamma", b"added").unwrap();
    assert_eq!(store.indexed_get(b"alpha").unwrap(), Some(b"new".to_vec()));
    assert_eq!(store.indexed_get(b"beta").unwrap(), None);
    assert_eq!(store.indexed_range(b"a", b"z").unwrap(),
        vec![(b"alpha".to_vec(), b"new".to_vec()), (b"gamma".to_vec(), b"added".to_vec())]);
    store.checkpoint().unwrap();
    assert_eq!(store.indexed_get(b"alpha").unwrap(), Some(b"new".to_vec()));
    assert_eq!(store.indexed_get(b"beta").unwrap(), None);
    assert_eq!(store.indexed_get(b"gamma").unwrap(), Some(b"added".to_vec()));
}

#[test]
fn damaged_published_index_reports_failure_not_missing_row() {
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"security", b"keep").unwrap();
        store.checkpoint().unwrap();
    }
    let path = dir.path().join("index-1.bri");
    let mut bytes = fs::read(&path).unwrap();
    bytes[4096 + 50] ^= 0x1;
    fs::write(path, bytes).unwrap();
    let store = RowStore::open(dir.path()).unwrap();
    assert_eq!(store.get(b"security"), Some(b"keep".to_vec()));
    assert!(matches!(store.indexed_get(b"security"), Err(StoreError::Index(_))));
}

#[test]
fn old_index_is_pruned_only_after_new_checkpoint_published() {
    let dir = tempdir().unwrap();
    let store = RowStore::open(dir.path()).unwrap();
    store.put(b"one", b"first").unwrap();
    store.checkpoint().unwrap();
    assert!(dir.path().join("index-1.bri").exists());
    store.put(b"two", b"second").unwrap();
    store.checkpoint().unwrap();
    assert!(!dir.path().join("index-1.bri").exists());
    assert!(dir.path().join("index-2.bri").exists());
}


#[test]
fn wal_delta_point_reads_preserve_checkpoint_cache_and_new_mutations() {
    let dir = tempdir().unwrap();
    let store = RowStore::open(dir.path()).unwrap();
    for i in 0..240 {
        store.put(format!("key-{i:04}").as_bytes(), b"checkpoint").unwrap();
    }
    store.checkpoint().unwrap();
    assert_eq!(store.uncheckpointed_index_changes(), 0);
    assert_eq!(store.indexed_get(b"key-0100").unwrap(), Some(b"checkpoint".to_vec()));
    let (_, snapshot) = store.index_cache_stats().unwrap();
    assert!(snapshot.is_some());

    store.put(b"key-0001", b"modified").unwrap();
    store.put(b"key-0240", b"inserted").unwrap();
    store.delete(b"key-0100").unwrap();
    assert_eq!(store.uncheckpointed_index_changes(), 3);
    assert_eq!(store.indexed_get(b"key-0001").unwrap(), Some(b"modified".to_vec()));
    assert_eq!(store.indexed_get(b"key-0100").unwrap(), None);
    assert_eq!(store.indexed_get(b"key-0240").unwrap(), Some(b"inserted".to_vec()));
    assert_eq!(store.indexed_get(b"key-0239").unwrap(), Some(b"checkpoint".to_vec()));
    assert_eq!(store.indexed_get(b"key-0239").unwrap(), Some(b"checkpoint".to_vec()));

    // Since unchanged rows are still accessed via the checkpoint tree, the
    // cache survives WAL changes rather than resetting to memory-only reads.
    let (index, snapshot) = store.index_cache_stats().unwrap();
    assert!(index.hits > 0);
    assert!(snapshot.unwrap().hits > 0);
}

#[test]
fn wal_delta_merges_insert_replace_and_delete_in_range() {
    let dir = tempdir().unwrap();
    let store = RowStore::open(dir.path()).unwrap();
    for i in 0..250 {
        store.put(format!("entry-{i:04}").as_bytes(), format!("v{i}").as_bytes()).unwrap();
    }
    store.checkpoint().unwrap();
    store.put(b"entry-0000", b"changed").unwrap();
    store.delete(b"entry-0030").unwrap();
    store.put(b"entry-0250", b"appended").unwrap();
    store.put(b"entry-0100", b"replaced").unwrap();
    store.delete(b"entry-0030").unwrap(); // repeating a tombstone remains idempotent
    assert_eq!(store.uncheckpointed_index_changes(), 4);

    let merged = store.indexed_range(b"entry-0000", b"entry-0251").unwrap();
    assert_eq!(merged.len(), 250);
    assert_eq!(merged.first().unwrap().1, b"changed".to_vec());
    assert!(!merged.iter().any(|(k, _)| k == b"entry-0030"));
    assert!(merged.iter().any(|(k,v)| k == b"entry-0100" && v == b"replaced"));
    assert_eq!(merged.last().unwrap(), &(b"entry-0250".to_vec(), b"appended".to_vec()));
}

#[test]
fn wal_delta_reconstructed_from_log_after_restart() {
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"alpha", b"old").unwrap();
        store.put(b"beta", b"keep").unwrap();
        store.checkpoint().unwrap();
        store.put(b"alpha", b"new").unwrap();
        store.delete(b"beta").unwrap();
        store.put(b"gamma", b"inserted").unwrap();
        assert_eq!(store.uncheckpointed_index_changes(), 3);
    }
    let store = RowStore::open(dir.path()).unwrap();
    assert_eq!(store.uncheckpointed_index_changes(), 3);
    assert_eq!(store.indexed_get(b"alpha").unwrap(), Some(b"new".to_vec()));
    assert_eq!(store.indexed_get(b"beta").unwrap(), None);
    assert_eq!(store.indexed_get(b"gamma").unwrap(), Some(b"inserted".to_vec()));
    assert_eq!(store.indexed_range(b"a", b"z").unwrap(), vec![
        (b"alpha".to_vec(), b"new".to_vec()),
        (b"gamma".to_vec(), b"inserted".to_vec()),
    ]);
    store.checkpoint().unwrap();
    assert_eq!(store.uncheckpointed_index_changes(), 0);
    drop(store);
    let again = RowStore::open(dir.path()).unwrap();
    assert_eq!(again.uncheckpointed_index_changes(), 0);
    assert_eq!(again.indexed_get(b"alpha").unwrap(), Some(b"new".to_vec()));
    assert_eq!(again.indexed_get(b"beta").unwrap(), None);
}

#[test]
fn atomic_batch_updates_join_wal_delta_as_one_commit() {
    use bradian_relational::row_store::RowMutation;
    let dir = tempdir().unwrap();
    let store = RowStore::open(dir.path()).unwrap();
    store.put(b"one", b"1").unwrap();
    store.put(b"two", b"2").unwrap();
    store.checkpoint().unwrap();
    let sequence = store.commit_batch(Some(store.last_lsn()), &[
        RowMutation::Put { key: b"one".to_vec(), value: b"updated".to_vec() },
        RowMutation::Delete { key: b"two".to_vec() },
        RowMutation::Put { key: b"three".to_vec(), value: b"3".to_vec() },
    ]).unwrap();
    assert_eq!(sequence, 3);
    assert_eq!(store.uncheckpointed_index_changes(), 3);
    assert_eq!(store.indexed_get(b"one").unwrap(), Some(b"updated".to_vec()));
    assert_eq!(store.indexed_get(b"two").unwrap(), None);
    assert_eq!(store.indexed_get(b"three").unwrap(), Some(b"3".to_vec()));
    assert_eq!(store.indexed_range(b"a", b"z").unwrap(), vec![
        (b"one".to_vec(), b"updated".to_vec()),
        (b"three".to_vec(), b"3".to_vec()),
    ]);
}
