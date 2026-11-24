//! ADR 0308 — Buffer-pool / dirty-page-manager **skeleton**.
//!
//! Sits one layer above the existing `mutable_page` file
//! allocator (ADR 0096). This is the typed surface future
//! P2 slices will fill in — WAL-protected dirty-page
//! writes, LRU-bounded page replacement, multi-writer
//! concurrency. The in-memory backing shipped here
//! exercises the invariants (monotonic per-page LSN fence,
//! globally advancing checkpoint LSN) end-to-end today.
//!
//! **Honest scope**: no disk pages (`mutable_page` is the
//! real file allocator; this is the manager layer over
//! it), no page replacement policy, no multi-writer
//! concurrency beyond the global mutex. Don't claim the
//! buffer manager is implemented because this skeleton
//! exists.

use parking_lot::Mutex;
use std::collections::BTreeMap;

use crate::row_store::{StoreError, StoreResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct PageId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub id:    PageId,
    pub lsn:   u64,
    pub bytes: Vec<u8>,
}

/// Minimum surface future slices must honour.
pub trait PageStore {
    fn allocate(&self)             -> StoreResult<PageId>;
    fn read(&self,  id: PageId)    -> StoreResult<Page>;
    fn write(&self, page: Page)    -> StoreResult<()>;
    fn flush(&self, up_to_lsn: u64) -> StoreResult<()>;
    fn checkpoint(&self)           -> StoreResult<u64>;
}

#[derive(Debug, Default)]
struct Inner {
    pages:         BTreeMap<PageId, Page>,
    next_page:     u64,
    global_lsn:    u64,
    wal:           Vec<(PageId, u64)>,   // (page, assigned lsn) — bookkeeping only
    flush_calls:   u64,
    last_checkpoint_lsn: u64,
}

#[derive(Debug, Default)]
pub struct InMemoryPageStore {
    inner: Mutex<Inner>,
}

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
        g.wal.push((page.id, page.lsn));
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
        g.wal.clear();   // conceptual WAL truncation
        Ok(g.global_lsn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocate_write_read_round_trips_a_page() {
        let store = InMemoryPageStore::new();
        let id = store.allocate().unwrap();
        store.write(Page { id, lsn: 1, bytes: b"hello".to_vec() }).unwrap();
        let got = store.read(id).unwrap();
        assert_eq!(got.bytes, b"hello");
        assert_eq!(got.lsn, 1);
    }

    #[test]
    fn write_with_non_advancing_lsn_is_rejected() {
        let store = InMemoryPageStore::new();
        let id = store.allocate().unwrap();
        store.write(Page { id, lsn: 5, bytes: vec![1] }).unwrap();
        let err = store.write(Page { id, lsn: 5, bytes: vec![2] }).unwrap_err();
        assert!(matches!(err, StoreError::Corruption(_)));
        let err = store.write(Page { id, lsn: 3, bytes: vec![3] }).unwrap_err();
        assert!(matches!(err, StoreError::Corruption(_)));
        // Advancing is fine.
        store.write(Page { id, lsn: 10, bytes: vec![4] }).unwrap();
        assert_eq!(store.read(id).unwrap().lsn, 10);
    }

    #[test]
    fn checkpoint_advances_global_lsn_and_flush_is_counted() {
        let store = InMemoryPageStore::new();
        let id = store.allocate().unwrap();
        store.write(Page { id, lsn: 7, bytes: vec![1] }).unwrap();
        assert_eq!(store.global_lsn(), 7);
        let ck1 = store.checkpoint().unwrap();
        assert!(ck1 > 7, "checkpoint must advance beyond prior global LSN");
        store.flush(ck1).unwrap();
        store.flush(ck1).unwrap();
        assert_eq!(store.flush_calls(), 2);
        let ck2 = store.checkpoint().unwrap();
        assert!(ck2 > ck1);
    }
}
