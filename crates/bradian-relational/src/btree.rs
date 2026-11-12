//! Immutable, disk-resident B+tree for BRPG row checkpoint pages.
//!
//! The index maps bytewise-ordered keys to physical checkpoint page locations.
//! Nodes are fixed 4 KiB CRC-protected BRPG pages. Internal separators identify
//! the lowest key in each right child. Leaf pages link forward for range scans.
//! This is a *derived* index: the row checkpoint and WAL are authoritative.
//! Building the index never modifies existing published index files.
//!
//! This module does not implement in-place split, page reuse, or an MVCC buffer
//! manager. It does implement a real multi-level page tree with on-disk binary
//! search and corruption validation. All IDs and lengths are little-endian.

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use thiserror::Error;

use crate::page::{Page, PageError, PageKind, MAX_PAYLOAD_SIZE, PAGE_SIZE};
use crate::buffer_pool::{CacheError, CacheStats, PageCache};

const MAGIC: &[u8; 4] = b"BRIX";
const VERSION: u16 = 1;
const META_SIZE: usize = 30;
const NODE_HEADER: usize = 11;
const LEAF: u8 = 1;
const INTERNAL: u8 = 2;
const MAX_KEY: usize = 512;

pub(crate) type LeafEntries = Vec<(Vec<u8>, RowPointer)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowPointer {
    pub page_id: u64,
    pub offset: u16,
}

#[derive(Debug, Error)]
pub enum IndexError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Page(#[from] PageError),
    #[error(transparent)]
    Cache(#[from] CacheError),
    #[error("B+tree index is corrupt: {0}")]
    Corruption(&'static str),
    #[error("invalid B+tree index input: {0}")]
    Invalid(&'static str),
}

pub type IndexResult<T> = Result<T, IndexError>;

#[derive(Debug)]
struct NodeDescriptor {
    id: u64,
    first: Vec<u8>,
}

#[derive(Debug)]
pub struct DiskBTree {
    cache: PageCache,
    root: u64,
    pages: u64,
    entries: u64,
}

impl DiskBTree {
    /// Build a complete immutable page tree at a new, unpublished path.
    /// Callers must synchronize and atomically rename the file before use.
    pub fn build(path: &Path, lsn: u64, entries: &[(Vec<u8>, RowPointer)]) -> IndexResult<()> {
        if path.exists() {
            return Err(IndexError::Invalid("index build target already exists"));
        }
        for (i, (key, ptr)) in entries.iter().enumerate() {
            if key.is_empty() || key.len() > MAX_KEY || ptr.page_id == 0
                || ptr.offset as usize >= MAX_PAYLOAD_SIZE {
                return Err(IndexError::Invalid("invalid key or row pointer"));
            }
            if i > 0 && entries[i - 1].0 >= *key {
                return Err(IndexError::Invalid("keys must be strictly increasing"));
            }
        }
        let mut file = OpenOptions::new().create_new(true).write(true).read(true).open(path)?;
        file.write_all(&[0_u8; PAGE_SIZE])?; // metadata is finalized last
        let mut descriptors = Vec::<NodeDescriptor>::new();
        let mut next_id = 1_u64;
        if !entries.is_empty() {
            let mut start = 0;
            while start < entries.len() {
                let first = entries[start].0.clone();
                let mut payload = vec![LEAF, 0, 0];
                payload.extend_from_slice(&0_u64.to_le_bytes()); // next leaf
                let mut count = 0_u16;
                while start < entries.len() {
                    let (key, ptr) = &entries[start];
                    let len = 2 + 8 + 2 + key.len();
                    if payload.len() + len > MAX_PAYLOAD_SIZE {
                        break;
                    }
                    payload.extend_from_slice(&(key.len() as u16).to_le_bytes());
                    payload.extend_from_slice(&ptr.page_id.to_le_bytes());
                    payload.extend_from_slice(&ptr.offset.to_le_bytes());
                    payload.extend_from_slice(key);
                    count += 1;
                    start += 1;
                }
                if count == 0 {
                    return Err(IndexError::Invalid("leaf key cannot fit in a page"));
                }
                payload[1..3].copy_from_slice(&count.to_le_bytes());
                let successor = if start < entries.len() { next_id + 1 } else { 0 };
                payload[3..11].copy_from_slice(&successor.to_le_bytes());
                file.write_all(&Page::new(next_id, lsn, PageKind::Index, payload)?.encode()?)?;
                descriptors.push(NodeDescriptor { id: next_id, first });
                next_id += 1;
            }
            // Promote each level until a single root is produced. A descriptor's
            // first key is the minimum key in its complete child subtree.
            while descriptors.len() > 1 {
                let mut parents = Vec::<NodeDescriptor>::new();
                let mut start = 0;
                while start < descriptors.len() {
                    let first = descriptors[start].first.clone();
                    let first_child = descriptors[start].id;
                    let mut payload = vec![INTERNAL, 0, 0];
                    payload.extend_from_slice(&first_child.to_le_bytes());
                    start += 1;
                    let mut count = 0_u16;
                    while start < descriptors.len() {
                        let descriptor = &descriptors[start];
                        let len = 2 + 8 + descriptor.first.len();
                        if payload.len() + len > MAX_PAYLOAD_SIZE {
                            break;
                        }
                        payload.extend_from_slice(&(descriptor.first.len() as u16).to_le_bytes());
                        payload.extend_from_slice(&descriptor.id.to_le_bytes());
                        payload.extend_from_slice(&descriptor.first);
                        count += 1;
                        start += 1;
                    }
                    payload[1..3].copy_from_slice(&count.to_le_bytes());
                    file.write_all(&Page::new(next_id, lsn, PageKind::Index, payload)?.encode()?)?;
                    parents.push(NodeDescriptor { id: next_id, first });
                    next_id += 1;
                }
                descriptors = parents;
            }
        }
        let root = descriptors.first().map_or(0, |d| d.id);
        let pages = next_id - 1;
        let mut meta = Vec::with_capacity(META_SIZE);
        meta.extend_from_slice(MAGIC);
        meta.extend_from_slice(&VERSION.to_le_bytes());
        meta.extend_from_slice(&root.to_le_bytes());
        meta.extend_from_slice(&pages.to_le_bytes());
        meta.extend_from_slice(&(entries.len() as u64).to_le_bytes());
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&Page::new(0, lsn, PageKind::Metadata, meta)?.encode()?)?;
        file.sync_all()?;
        Ok(())
    }

    pub fn open(path: &Path, expected_lsn: u64) -> IndexResult<Self> {
        Self::open_with_capacity(path, expected_lsn, 128)
    }

    /// Open an immutable index with an explicit maximum resident page count.
    pub fn open_with_capacity(
        path: &Path,
        expected_lsn: u64,
        capacity: usize,
    ) -> IndexResult<Self> {
        let mut cache = PageCache::open(path, expected_lsn, capacity)?;
        let meta = cache.read(0)?;
        if meta.kind != PageKind::Metadata || meta.payload.len() != META_SIZE ||
            &meta.payload[..4] != MAGIC ||
            u16::from_le_bytes(meta.payload[4..6].try_into().unwrap()) != VERSION
        {
            return Err(IndexError::Corruption("invalid index metadata"));
        }
        let root = u64::from_le_bytes(meta.payload[6..14].try_into().unwrap());
        let pages = u64::from_le_bytes(meta.payload[14..22].try_into().unwrap());
        let entries = u64::from_le_bytes(meta.payload[22..30].try_into().unwrap());
        if pages.checked_add(1) != Some(cache.page_count())
            || (root == 0) != (entries == 0) || root > pages
            || (entries == 0 && pages != 0)
        {
            return Err(IndexError::Corruption("index metadata and file layout disagree"));
        }
        Ok(Self { cache, root, pages, entries })
    }

    pub fn len(&self) -> u64 { self.entries }
    pub fn is_empty(&self) -> bool { self.entries == 0 }
    pub fn cache_stats(&self) -> CacheStats { self.cache.stats() }

    fn page(&mut self, id: u64) -> IndexResult<Page> {
        if id == 0 || id > self.pages {
            return Err(IndexError::Corruption("page reference outside index file"));
        }
        let page = self.cache.read(id)?;
        if page.kind != PageKind::Index || page.payload.len() < NODE_HEADER {
            return Err(IndexError::Corruption("invalid index page header"));
        }
        Ok(page)
    }

    /// On-disk point query; visits at most one node per tree level.
    pub fn get(&mut self, key: &[u8]) -> IndexResult<Option<RowPointer>> {
        if self.root == 0 { return Ok(None); }
        let leaf = self.seek_leaf(key)?;
        for (entry, ptr) in read_leaf(&leaf.payload)?.0 {
            match entry.as_slice().cmp(key) {
                std::cmp::Ordering::Equal => return Ok(Some(ptr)),
                std::cmp::Ordering::Greater => return Ok(None),
                std::cmp::Ordering::Less => {}
            }
        }
        Ok(None)
    }

    /// Inclusive lower bound, exclusive upper bound. Returns ordered row
    /// pointers without loading the complete B+tree into process memory.
    pub fn range(&mut self, start: &[u8], end: &[u8]) -> IndexResult<Vec<(Vec<u8>, RowPointer)>> {
        if start >= end || self.root == 0 { return Ok(Vec::new()); }
        let leaf = self.seek_leaf(start)?;
        let mut current_id = leaf.page_id;
        let mut found = Vec::new();
        let mut visited = 0_u64;
        let mut last_key: Option<Vec<u8>> = None;
        loop {
            visited += 1;
            if visited > self.pages { return Err(IndexError::Corruption("leaf chain contains a cycle")); }
            let page = if current_id == leaf.page_id && visited == 1 {
                leaf.clone()
            } else { self.page(current_id)? };
            let (rows, next) = read_leaf(&page.payload)?;
            for (key, pointer) in rows {
                if last_key.as_ref().is_some_and(|previous| previous >= &key) {
                    return Err(IndexError::Corruption("out-of-order leaf chain"));
                }
                last_key = Some(key.clone());
                if key.as_slice() >= end { return Ok(found); }
                if key.as_slice() >= start { found.push((key, pointer)); }
            }
            if next == 0 { break; }
            if next <= current_id || next > self.pages {
                return Err(IndexError::Corruption("invalid leaf chain"));
            }
            current_id = next;
        }
        Ok(found)
    }

    fn seek_leaf(&mut self, key: &[u8]) -> IndexResult<Page> {
        let mut id = self.root;
        let mut depth = 0_u64;
        loop {
            depth += 1;
            if depth > self.pages {
                return Err(IndexError::Corruption("index tree contains a cycle"));
            }
            let page = self.page(id)?;
            match page.payload[0] {
                LEAF => return Ok(page),
                INTERNAL => { id = read_internal(&page.payload, key)?; }
                _ => return Err(IndexError::Corruption("unrecognized index node type")),
            }
        }
    }
}

fn read_leaf(payload: &[u8]) -> IndexResult<(LeafEntries, u64)> {
    if payload.len() < NODE_HEADER || payload[0] != LEAF {
        return Err(IndexError::Corruption("not a leaf node"));
    }
    let count = u16::from_le_bytes(payload[1..3].try_into().unwrap()) as usize;
    let next = u64::from_le_bytes(payload[3..11].try_into().unwrap());
    if count == 0 { return Err(IndexError::Corruption("empty leaf node")); }
    let mut rows = Vec::with_capacity(count);
    let mut pos = NODE_HEADER;
    for _ in 0..count {
        if payload.len() - pos < 12 { return Err(IndexError::Corruption("truncated leaf entry")); }
        let len = u16::from_le_bytes(payload[pos..pos+2].try_into().unwrap()) as usize;
        let page_id = u64::from_le_bytes(payload[pos+2..pos+10].try_into().unwrap());
        let offset = u16::from_le_bytes(payload[pos+10..pos+12].try_into().unwrap());
        pos += 12;
        if len == 0 || len > MAX_KEY || len > payload.len() - pos || page_id == 0 {
            return Err(IndexError::Corruption("invalid leaf entry"));
        }
        let key = payload[pos..pos+len].to_vec();
        if rows.last().is_some_and(|(previous, _): &(Vec<u8>, RowPointer)| previous >= &key) {
            return Err(IndexError::Corruption("unordered leaf entries"));
        }
        rows.push((key, RowPointer { page_id, offset }));
        pos += len;
    }
    if pos != payload.len() { return Err(IndexError::Corruption("trailing leaf payload")); }
    Ok((rows, next))
}

fn read_internal(payload: &[u8], key: &[u8]) -> IndexResult<u64> {
    if payload.len() < NODE_HEADER || payload[0] != INTERNAL {
        return Err(IndexError::Corruption("not an internal node"));
    }
    let count = u16::from_le_bytes(payload[1..3].try_into().unwrap()) as usize;
    let mut target = u64::from_le_bytes(payload[3..11].try_into().unwrap());
    if target == 0 { return Err(IndexError::Corruption("zero leftmost child")); }
    let mut pos = NODE_HEADER;
    let mut previous: Option<&[u8]> = None;
    for _ in 0..count {
        if payload.len() - pos < 10 { return Err(IndexError::Corruption("truncated branch entry")); }
        let len = u16::from_le_bytes(payload[pos..pos+2].try_into().unwrap()) as usize;
        let child = u64::from_le_bytes(payload[pos+2..pos+10].try_into().unwrap());
        pos += 10;
        if len == 0 || len > MAX_KEY || len > payload.len() - pos || child == 0 {
            return Err(IndexError::Corruption("invalid branch entry"));
        }
        let separator = &payload[pos..pos+len];
        if previous.is_some_and(|p| p >= separator) {
            return Err(IndexError::Corruption("unordered separators"));
        }
        previous = Some(separator);
        if key >= separator { target = child; }
        pos += len;
    }
    if pos != payload.len() { return Err(IndexError::Corruption("trailing branch payload")); }
    Ok(target)
}
