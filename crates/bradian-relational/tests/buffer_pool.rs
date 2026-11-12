//! Physical buffer pool regression tests using actual BRPG files.

use std::fs;
use std::io::Write;

use bradian_relational::buffer_pool::{CacheError, PageCache};
use bradian_relational::page::{Page, PageKind, PAGE_SIZE};
use bradian_relational::row_store::RowStore;
use tempfile::tempdir;

fn build_pages(path: &std::path::Path, lsn: u64, pages: usize) {
    let mut file = fs::File::create(path).unwrap();
    for i in 0..pages {
        let page = Page::new(i as u64, lsn, PageKind::Data, vec![i as u8; 64]).unwrap();
        file.write_all(&page.encode().unwrap()).unwrap();
    }
    file.sync_all().unwrap();
}

#[test]
fn bounded_cache_eviction_is_least_recently_used() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("pages.brp");
    build_pages(&path, 77, 4);
    let mut cache = PageCache::open(&path, 77, 2).unwrap();
    assert_eq!(cache.read(0).unwrap().page_id, 0); // miss
    assert_eq!(cache.read(1).unwrap().page_id, 1); // miss
    assert_eq!(cache.read(0).unwrap().page_id, 0); // hit, most recent
    assert_eq!(cache.read(2).unwrap().page_id, 2); // evict 1
    let stats = cache.stats();
    assert_eq!(stats.capacity, 2);
    assert_eq!(stats.resident, 2);
    assert_eq!(stats.hits, 1);
    assert_eq!(stats.misses, 3);
    assert_eq!(stats.evictions, 1);
    cache.read(1).unwrap(); // miss again, evict 0
    assert_eq!(cache.stats().evictions, 2);
    assert_eq!(cache.stats().misses, 4);
}

#[test]
fn buffer_cache_rejects_invalid_capacity_pages_and_generations() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("pages.brp");
    build_pages(&path, 101, 3);
    assert!(matches!(PageCache::open(&path, 101, 0), Err(CacheError::Invalid(_))));
    let mut pool = PageCache::open(&path, 101, 1).unwrap();
    assert!(matches!(pool.read(3), Err(CacheError::Corruption(_))));
    assert!(matches!(pool.read(u64::MAX), Err(CacheError::Corruption(_))));
    let mut wrong_generation = PageCache::open(&path, 202, 3).unwrap();
    assert!(matches!(wrong_generation.read(1), Err(CacheError::Corruption(_))));
}

#[test]
fn corrupted_uncached_page_never_enters_pool() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("pages.brp");
    build_pages(&path, 9, 3);
    let mut bytes = fs::read(&path).unwrap();
    bytes[PAGE_SIZE * 2 + 38] ^= 0x5a;
    fs::write(&path, bytes).unwrap();
    let mut pool = PageCache::open(&path, 9, 2).unwrap();
    assert!(matches!(pool.read(2), Err(CacheError::Page(_))));
    assert_eq!(pool.stats().resident, 0);
    assert_eq!(pool.stats().misses, 0);
    assert_eq!(pool.read(0).unwrap().page_id, 0);
    assert_eq!(pool.stats().resident, 1);
}

#[test]
fn physical_page_identity_tampering_is_rejected_even_after_recomputed_crc() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("pages.brp");
    build_pages(&path, 1, 2);
    let mut bytes = fs::read(&path).unwrap();
    // Copy the entire valid first page into the position for page 1. Its CRC
    // remains valid, but page ID no longer agrees with its physical offset.
    let duplicate = bytes[..PAGE_SIZE].to_vec();
    bytes[PAGE_SIZE..2*PAGE_SIZE].copy_from_slice(&duplicate);
    fs::write(&path, bytes).unwrap();
    let mut pool = PageCache::open(&path, 1, 2).unwrap();
    assert!(matches!(pool.read(1), Err(CacheError::Corruption(_))));
}

#[test]
fn page_cache_disallows_non_page_aligned_files() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("short.brp");
    fs::write(&path, [0_u8; 123]).unwrap();
    assert!(matches!(PageCache::open(&path, 1, 2), Err(CacheError::Corruption(_))));
}

#[test]
fn indexed_row_store_reuses_generation_caches_and_invalidates_on_checkpoint() {
    let dir = tempdir().unwrap();
    let store = RowStore::open(dir.path()).unwrap();
    for i in 0..200 {
        store.put(format!("key-{i:04}").as_bytes(), vec![i as u8; 64].as_slice()).unwrap();
    }
    store.checkpoint().unwrap();
    assert!(store.index_cache_stats().is_none());
    for _ in 0..8 {
        assert_eq!(store.indexed_get(b"key-0001").unwrap(), Some(vec![1; 64]));
        assert_eq!(store.indexed_get(b"key-0020").unwrap(), Some(vec![20; 64]));
    }
    let (index, snapshot) = store.index_cache_stats().unwrap();
    assert!(index.hits > 0, "root/leaf pages should be reused");
    assert!(snapshot.unwrap().hits > 0, "data pages should be reused");
    assert!(index.resident <= index.capacity);

    store.put(b"key-0001", b"changed").unwrap();
    assert_eq!(store.indexed_get(b"key-0001").unwrap(), Some(b"changed".to_vec()));
    store.checkpoint().unwrap();
    assert!(store.index_cache_stats().is_none(), "new checkpoint must invalidate caches");
    assert_eq!(store.indexed_get(b"key-0001").unwrap(), Some(b"changed".to_vec()));
}
