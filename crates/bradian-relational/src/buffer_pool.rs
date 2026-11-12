//! Bounded read-only buffer pool for immutable BRPG page generations.
//!
//! Bradian's row checkpoints and derived B+tree indexes are immutable after
//! publication. This cache holds decoded, checksum-verified physical pages for
//! those generations. Every cache miss validates the page ID, LSN and CRC;
//! cached hits are safe only because published page files must never mutate.
//!
//! A future writable buffer pool requires a separate pin/latch/dirty-page/WAL
//! protocol. This module intentionally does not claim write-back safety.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use thiserror::Error;

use crate::page::{Page, PageError, PAGE_SIZE};

#[derive(Debug, Error)]
pub enum CacheError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Page(#[from] PageError),
    #[error("invalid buffer pool: {0}")]
    Invalid(&'static str),
    #[error("physical page integrity failure: {0}")]
    Corruption(&'static str),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub resident: usize,
    pub capacity: usize,
}

#[derive(Debug)]
struct Entry {
    page: Page,
    accessed: u64,
}

/// A bounded LRU cache tied to one immutable physical file generation.
///
/// Not thread-safe by itself. The owning DiskBTree or RowStore serializes
/// access; there is no shared mutable file cursor across independent stores.
#[derive(Debug)]
pub struct PageCache {
    file: File,
    generation: u64,
    pages: u64,
    capacity: usize,
    clock: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
    resident: BTreeMap<u64, Entry>,
}

impl PageCache {
    pub fn open(path: &Path, generation: u64, capacity: usize) -> Result<Self, CacheError> {
        if capacity == 0 {
            return Err(CacheError::Invalid("page cache capacity must be positive"));
        }
        let file = File::open(path)?;
        let length = file.metadata()?.len();
        if length < PAGE_SIZE as u64 || length % PAGE_SIZE as u64 != 0 {
            return Err(CacheError::Corruption("physical file length is not page aligned"));
        }
        Ok(Self {
            file,
            generation,
            pages: length / PAGE_SIZE as u64,
            capacity,
            clock: 0,
            hits: 0,
            misses: 0,
            evictions: 0,
            resident: BTreeMap::new(),
        })
    }

    pub fn page_count(&self) -> u64 { self.pages }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits,
            misses: self.misses,
            evictions: self.evictions,
            resident: self.resident.len(),
            capacity: self.capacity,
        }
    }

    /// Return a verified, owned physical page. No reference to an evictable
    /// entry escapes the cache, so callers cannot hold stale cache borrows.
    pub fn read(&mut self, page_id: u64) -> Result<Page, CacheError> {
        if page_id >= self.pages {
            return Err(CacheError::Corruption("page ID exceeds physical file length"));
        }
        self.clock = self.clock.wrapping_add(1);
        if self.clock == 0 {
            // Rebase LRU values on wrap-around. The exact order is preserved.
            let mut order = self.resident.iter().map(|(id, e)| (*id, e.accessed))
                .collect::<Vec<_>>();
            order.sort_by_key(|(_, used)| *used);
            for (rank, (id, _)) in order.into_iter().enumerate() {
                self.resident.get_mut(&id).expect("resident page").accessed = rank as u64;
            }
            self.clock = self.resident.len() as u64 + 1;
        }
        if let Some(entry) = self.resident.get_mut(&page_id) {
            self.hits += 1;
            entry.accessed = self.clock;
            return Ok(entry.page.clone());
        }

        let offset = page_id.checked_mul(PAGE_SIZE as u64)
            .ok_or(CacheError::Corruption("page offset overflow"))?;
        self.file.seek(SeekFrom::Start(offset))?;
        let mut bytes = [0_u8; PAGE_SIZE];
        self.file.read_exact(&mut bytes)?;
        let page = Page::decode(&bytes)?;
        if page.page_id != page_id || page.lsn != self.generation {
            return Err(CacheError::Corruption("page ID or generation mismatch"));
        }
        // A corrupted page must not be cached or returned.
        self.misses += 1;
        if self.resident.len() == self.capacity {
            let old = self.resident.iter()
                .min_by_key(|(id, entry)| (entry.accessed, **id))
                .map(|(id, _)| *id)
                .expect("full cache has a victim");
            self.resident.remove(&old);
            self.evictions += 1;
        }
        self.resident.insert(page_id, Entry { page: page.clone(), accessed: self.clock });
        Ok(page)
    }
}
