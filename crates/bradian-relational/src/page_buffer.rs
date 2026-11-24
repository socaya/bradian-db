//! ADR 0308 — Buffer-pool / dirty-page-manager.
//!
//! Sits one layer above the existing `mutable_page`
//! allocator (ADR 0096). `DiskPageBuffer` wraps a
//! `MutablePageFile` with:
//!
//! - A **bounded LRU pool** of cached pages.
//! - **Dirty tracking** per page_id.
//! - **Per-page monotonic LSN fence** (an older LSN write
//!   is refused).
//! - **Evict-flush** semantics: evicting a dirty page
//!   writes it to the allocator first.
//! - **`flush` / `checkpoint`** that persist all dirty
//!   pages and advance a global LSN counter.
//!
//! The in-memory `InMemoryPageStore` is preserved for
//! tests that don't need disk.

use std::collections::{HashMap, VecDeque};
use std::num::NonZeroUsize;
use std::path::Path;

use parking_lot::Mutex;

use crate::mutable_page::{MutablePageFile, MutablePageError, PageKind};
use crate::row_store::{StoreError, StoreResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct PageId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub id:    PageId,
    pub lsn:   u64,
    pub bytes: Vec<u8>,
}

pub trait PageStore {
    fn allocate(&self)             -> StoreResult<PageId>;
    fn read(&self,  id: PageId)    -> StoreResult<Page>;
    fn write(&self, page: Page)    -> StoreResult<()>;
    fn flush(&self, up_to_lsn: u64) -> StoreResult<()>;
    fn checkpoint(&self)           -> StoreResult<u64>;
}

// ---------------------------------------------------------------------------
// In-memory variant — preserved for tests not needing disk.

#[derive(Debug, Default)]
struct InMemInner {
    pages:         std::collections::BTreeMap<PageId, Page>,
    next_page:     u64,
    global_lsn:    u64,
    flush_calls:   u64,
    last_checkpoint_lsn: u64,
}

#[derive(Debug, Default)]
pub struct InMemoryPageStore { inner: Mutex<InMemInner> }

impl InMemoryPageStore {
    pub fn new() -> Self { Self::default() }
    pub fn flush_calls(&self) -> u64 { self.inner.lock().flush_calls }
    pub fn global_lsn(&self)  -> u64 { self.inner.lock().global_lsn }
}

impl PageStore for InMemoryPageStore {
    fn allocate(&self) -> StoreResult<PageId> {
        let mut g = self.inner.lock();
        let id = PageId(g.next_page);
        g.next_page += 1;
        g.pages.insert(id, Page { id, lsn: 0, bytes: Vec::new() });
        Ok(id)
    }
    fn read(&self, id: PageId) -> StoreResult<Page> {
        self.inner.lock().pages.get(&id).cloned()
            .ok_or_else(|| StoreError::Corruption(format!("page {id:?} not allocated")))
    }
    fn write(&self, page: Page) -> StoreResult<()> {
        let mut g = self.inner.lock();
        let existing = g.pages.get(&page.id).ok_or_else(||
            StoreError::Corruption(format!("page {:?} not allocated", page.id)))?;
        if page.lsn <= existing.lsn && existing.lsn > 0 {
            return Err(StoreError::Corruption(format!(
                "page {:?} LSN must advance monotonically: new={} prior={}",
                page.id, page.lsn, existing.lsn)));
        }
        if page.lsn > g.global_lsn { g.global_lsn = page.lsn; }
        g.pages.insert(page.id, page);
        Ok(())
    }
    fn flush(&self, _up_to_lsn: u64) -> StoreResult<()> {
        self.inner.lock().flush_calls += 1;
        Ok(())
    }
    fn checkpoint(&self) -> StoreResult<u64> {
        let mut g = self.inner.lock();
        g.global_lsn += 1;
        g.last_checkpoint_lsn = g.global_lsn;
        Ok(g.global_lsn)
    }
}

// ---------------------------------------------------------------------------
// Disk-backed buffer pool.

#[derive(Debug)]
struct Cached {
    page:  Page,
    dirty: bool,
}

#[derive(Debug)]
struct PoolInner {
    cache:      HashMap<PageId, Cached>,
    lru:        VecDeque<PageId>,    // front = most recent, back = eviction candidate
    capacity:   usize,
    global_lsn: u64,
}

/// LRU-bounded buffer pool over a `MutablePageFile`.
/// Multi-writer safety is the single global mutex. Future
/// slices can shard the pool for concurrency.
#[derive(Debug)]
pub struct DiskPageBuffer {
    file:  MutablePageFile,
    inner: Mutex<PoolInner>,
}

impl DiskPageBuffer {
    /// Open or create the page file at `root` and wrap it
    /// with a buffer pool of `capacity` pages.
    pub fn open(root: impl AsRef<Path>, capacity: NonZeroUsize) -> StoreResult<Self> {
        let file = MutablePageFile::open(root).map_err(mpe_to_store)?;
        Ok(Self {
            file,
            inner: Mutex::new(PoolInner {
                cache:      HashMap::new(),
                lru:        VecDeque::new(),
                capacity:   capacity.get(),
                global_lsn: 0,
            }),
        })
    }

    pub fn global_lsn(&self) -> u64 { self.inner.lock().global_lsn }
    pub fn cache_len(&self)  -> usize { self.inner.lock().cache.len() }
    pub fn dirty_count(&self) -> usize {
        self.inner.lock().cache.values().filter(|c| c.dirty).count()
    }

    fn touch(inner: &mut PoolInner, id: PageId) {
        inner.lru.retain(|x| *x != id);
        inner.lru.push_front(id);
    }

    /// Evict the LRU entry if the pool is over capacity.
    /// A dirty evictee is flushed to the file first.
    fn evict_if_needed(file: &MutablePageFile, inner: &mut PoolInner) -> StoreResult<()> {
        while inner.cache.len() > inner.capacity {
            let Some(victim_id) = inner.lru.pop_back() else { break; };
            if let Some(entry) = inner.cache.remove(&victim_id) {
                if entry.dirty {
                    file.write_page(victim_id.0, PageKind::Opaque, &entry.page.bytes)
                        .map_err(mpe_to_store)?;
                }
            }
        }
        Ok(())
    }
}

impl PageStore for DiskPageBuffer {
    fn allocate(&self) -> StoreResult<PageId> {
        let raw = self.file.allocate().map_err(mpe_to_store)?;
        let id = PageId(raw);
        let mut g = self.inner.lock();
        g.cache.insert(id, Cached {
            page: Page { id, lsn: 0, bytes: Vec::new() }, dirty: false,
        });
        Self::touch(&mut g, id);
        Self::evict_if_needed(&self.file, &mut g)?;
        Ok(id)
    }

    fn read(&self, id: PageId) -> StoreResult<Page> {
        let mut g = self.inner.lock();
        if let Some(entry) = g.cache.get(&id) {
            let page = entry.page.clone();
            Self::touch(&mut g, id);
            return Ok(page);
        }
        // Miss — read from the file, insert into cache.
        drop(g);
        let (kind, bytes) = self.file.read_page(id.0).map_err(mpe_to_store)?;
        let _ = kind;
        let page = Page { id, lsn: 0, bytes };
        let mut g = self.inner.lock();
        g.cache.insert(id, Cached { page: page.clone(), dirty: false });
        Self::touch(&mut g, id);
        Self::evict_if_needed(&self.file, &mut g)?;
        Ok(page)
    }

    fn write(&self, page: Page) -> StoreResult<()> {
        let mut g = self.inner.lock();
        // Ensure the cache has the current state. If the
        // page isn't cached, load it first so the LSN
        // fence is checked against the real prior value.
        if !g.cache.contains_key(&page.id) {
            drop(g);
            let _ = self.read(page.id)?;   // populates cache
            g = self.inner.lock();
        }
        {
            let existing = g.cache.get(&page.id).ok_or_else(||
                StoreError::Corruption(format!("page {:?} not allocated", page.id)))?;
            if page.lsn <= existing.page.lsn && existing.page.lsn > 0 {
                return Err(StoreError::Corruption(format!(
                    "page {:?} LSN must advance monotonically: new={} prior={}",
                    page.id, page.lsn, existing.page.lsn)));
            }
        }
        if page.lsn > g.global_lsn { g.global_lsn = page.lsn; }
        let id = page.id;
        g.cache.insert(id, Cached { page, dirty: true });
        Self::touch(&mut g, id);
        Self::evict_if_needed(&self.file, &mut g)?;
        Ok(())
    }

    fn flush(&self, up_to_lsn: u64) -> StoreResult<()> {
        let g = self.inner.lock();
        // Collect dirty pages first so we can drop the lock
        // between writes (file has its own mutex).
        let candidates: Vec<(PageId, Vec<u8>)> = g.cache.iter()
            .filter(|(_, c)| c.dirty && c.page.lsn <= up_to_lsn)
            .map(|(id, c)| (*id, c.page.bytes.clone()))
            .collect();
        drop(g);
        for (id, bytes) in &candidates {
            self.file.write_page(id.0, PageKind::Opaque, bytes).map_err(mpe_to_store)?;
        }
        let mut g = self.inner.lock();
        for (id, _) in candidates {
            if let Some(entry) = g.cache.get_mut(&id) { entry.dirty = false; }
        }
        Ok(())
    }

    fn checkpoint(&self) -> StoreResult<u64> {
        // Flush all dirty pages regardless of LSN, then
        // advance the global LSN counter.
        self.flush(u64::MAX)?;
        let mut g = self.inner.lock();
        g.global_lsn += 1;
        Ok(g.global_lsn)
    }
}

fn mpe_to_store(e: MutablePageError) -> StoreError {
    match e {
        MutablePageError::Io(e) => StoreError::Io(e),
        other                   => StoreError::Corruption(format!("{other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // In-memory store tests (preserved from skeleton).

    #[test]
    fn allocate_write_read_round_trips_a_page_in_memory() {
        let store = InMemoryPageStore::new();
        let id = store.allocate().unwrap();
        store.write(Page { id, lsn: 1, bytes: b"hello".to_vec() }).unwrap();
        let got = store.read(id).unwrap();
        assert_eq!(got.bytes, b"hello");
        assert_eq!(got.lsn, 1);
    }

    #[test]
    fn write_with_non_advancing_lsn_is_rejected_in_memory() {
        let store = InMemoryPageStore::new();
        let id = store.allocate().unwrap();
        store.write(Page { id, lsn: 5, bytes: vec![1] }).unwrap();
        assert!(store.write(Page { id, lsn: 5, bytes: vec![2] }).is_err());
        assert!(store.write(Page { id, lsn: 3, bytes: vec![3] }).is_err());
        store.write(Page { id, lsn: 10, bytes: vec![4] }).unwrap();
        assert_eq!(store.read(id).unwrap().lsn, 10);
    }

    // Disk-backed buffer pool tests.

    fn open_buf(dir: &Path, cap: usize) -> DiskPageBuffer {
        DiskPageBuffer::open(dir, NonZeroUsize::new(cap).unwrap()).unwrap()
    }

    #[test]
    fn disk_buffer_round_trips_through_cache_and_fsynced_file() {
        let dir = tempfile::tempdir().unwrap();
        let buf = open_buf(dir.path(), 4);
        let id = buf.allocate().unwrap();
        buf.write(Page { id, lsn: 1, bytes: b"disk-content".to_vec() }).unwrap();
        assert_eq!(buf.dirty_count(), 1);
        // Read via cache — hit.
        let got = buf.read(id).unwrap();
        assert_eq!(got.bytes, b"disk-content");
        // Flush → dirty count drops, file now has the bytes.
        buf.flush(u64::MAX).unwrap();
        assert_eq!(buf.dirty_count(), 0);
    }

    #[test]
    fn disk_buffer_lsn_fence_matches_in_memory_variant() {
        let dir = tempfile::tempdir().unwrap();
        let buf = open_buf(dir.path(), 4);
        let id = buf.allocate().unwrap();
        buf.write(Page { id, lsn: 5, bytes: vec![1] }).unwrap();
        let err = buf.write(Page { id, lsn: 5, bytes: vec![2] }).unwrap_err();
        assert!(matches!(err, StoreError::Corruption(_)));
        let err = buf.write(Page { id, lsn: 3, bytes: vec![3] }).unwrap_err();
        assert!(matches!(err, StoreError::Corruption(_)));
        buf.write(Page { id, lsn: 10, bytes: vec![4] }).unwrap();
        assert_eq!(buf.read(id).unwrap().lsn, 10);
    }

    #[test]
    fn eviction_flushes_dirty_pages_to_disk_before_dropping_from_cache() {
        // Capacity 2: alloc 3 pages. The first one gets
        // evicted; its dirty bytes must land in the file so
        // a later read re-materialises them.
        let dir = tempfile::tempdir().unwrap();
        let buf = open_buf(dir.path(), 2);
        let a = buf.allocate().unwrap();
        buf.write(Page { id: a, lsn: 1, bytes: b"page-a-contents-are-persistent".to_vec() }).unwrap();
        let b = buf.allocate().unwrap();
        buf.write(Page { id: b, lsn: 2, bytes: b"page-b".to_vec() }).unwrap();
        // Allocating a third page pushes us over capacity.
        // The LRU entry (a) gets evicted. It was dirty, so
        // it must have been flushed first.
        let c = buf.allocate().unwrap();
        assert_ne!(c, a);
        buf.write(Page { id: c, lsn: 3, bytes: b"c".to_vec() }).unwrap();
        // Read `a` back — a cache miss that reloads from
        // the file. The bytes must match the pre-evict state.
        let re_a = buf.read(a).unwrap();
        assert_eq!(re_a.bytes, b"page-a-contents-are-persistent");
    }

    #[test]
    fn flush_persists_only_up_to_the_specified_lsn() {
        let dir = tempfile::tempdir().unwrap();
        let buf = open_buf(dir.path(), 8);
        let a = buf.allocate().unwrap();
        let b = buf.allocate().unwrap();
        buf.write(Page { id: a, lsn: 5,  bytes: b"A".to_vec() }).unwrap();
        buf.write(Page { id: b, lsn: 10, bytes: b"B".to_vec() }).unwrap();
        assert_eq!(buf.dirty_count(), 2);
        buf.flush(5).unwrap();
        // Only page `a` (lsn 5) should have been flushed.
        assert_eq!(buf.dirty_count(), 1);
        buf.flush(10).unwrap();
        assert_eq!(buf.dirty_count(), 0);
    }

    #[test]
    fn checkpoint_flushes_everything_and_advances_global_lsn() {
        let dir = tempfile::tempdir().unwrap();
        let buf = open_buf(dir.path(), 4);
        let a = buf.allocate().unwrap();
        buf.write(Page { id: a, lsn: 7, bytes: vec![1, 2, 3] }).unwrap();
        let before = buf.global_lsn();
        let ck = buf.checkpoint().unwrap();
        assert!(ck > before);
        assert_eq!(buf.dirty_count(), 0);
    }

    #[test]
    fn page_bytes_survive_reopen_after_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let id_u64 = {
            let buf = open_buf(dir.path(), 4);
            let id = buf.allocate().unwrap();
            buf.write(Page { id, lsn: 1,
                bytes: b"survives reopen".to_vec() }).unwrap();
            buf.checkpoint().unwrap();
            id.0
        };
        // Reopen — the mutable_page file holds the bytes.
        let buf = open_buf(dir.path(), 4);
        let got = buf.read(PageId(id_u64)).unwrap();
        assert_eq!(got.bytes, b"survives reopen");
    }
}
