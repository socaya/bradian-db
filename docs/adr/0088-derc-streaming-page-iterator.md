# ADR 0088 — `.derc` streaming page iterator (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-15 and implemented.**

Blanket pre-approval from the owner to continue the draft+
implement cycle for ADRs 0086–0088. Final member of that batch.

## 1. Context

- `SegmentReader::read_column(name)` materializes an entire
  column (all pages concatenated) into one `ColumnBuffer`.
- Large multi-page columns (ADR 0081+) require O(total-rows)
  memory peak.
- Callers that aggregate page-by-page (scan + filter +
  reduce pipelines) don't need the materialized whole; they
  want to consume pages as they come off disk.

## 2. Scope

**In scope:**

- `SegmentReader::iter_pages(name) -> ColumnPageIterator` —
  an iterator yielding one `BradianResult<ColumnBuffer>` per
  page in declared order.
- Each yielded page is a standalone `ColumnBuffer` (same
  shape `decode_page` would produce). v1.4 Dictionary pages
  include the column-dict attached per page (same as the
  ADR 0083 per-page shape; ADR 0087's single-clone fast path
  applies only to `read_column`, not the iterator).
- Iterator stops on error — once `Err` is yielded the next
  call returns `None`. Callers inspecting errors should
  break out on the first `Err`.
- Preserves all existing CRC, BACE/BACA, and dict-bounds
  checks per page.

**Explicitly out of scope** (future ADRs):

- **Lazy page reads at I/O level.** The iterator seeks +
  `read_exact` for each page; there's no mmap/prefetch.
- **Batched iteration** (e.g., N pages at a time).
- **Parallel iteration.** Callers who want parallelism
  materialize pages with `.collect()` then dispatch
  themselves.
- **Streaming writes.** Writer path unchanged.

## 3. Design

```rust
pub struct ColumnPageIterator<'r> {
    reader: &'r mut SegmentReader,
    col: ColumnMeta,
    cursor: usize,
    done: bool,
}

impl<'r> Iterator for ColumnPageIterator<'r> {
    type Item = BradianResult<ColumnBuffer>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.cursor >= self.col.pages.len() { return None; }
        let page = self.col.pages[self.cursor].clone();
        self.cursor += 1;
        let r = self.reader.read_page(&self.col, &page);
        if r.is_err() { self.done = true; }
        Some(r)
    }
}

impl SegmentReader {
    pub fn iter_pages(&mut self, name: &str) -> BradianResult<ColumnPageIterator<'_>> {
        let col = self.column(name)?.clone();
        Ok(ColumnPageIterator { reader: self, col, cursor: 0, done: false })
    }
}
```

## 3.5 Compliance

1. `iter_pages` yields exactly `col.pages.len()` pages for a
   healthy column.
2. Each yielded page's `len()` equals the declared
   `PageMeta.row_count`.
3. Concatenating all yielded pages via `concat_buffers` /
   `concat_dict_pages` produces the same `ColumnBuffer` as
   `read_column(name)`.
4. Iterator stops on first error.
5. v1.4 Dictionary column: each yielded page is a
   `DictIndex32` with the column-dict attached.
6. Non-dict columns: each yielded page is the expected
   variant (`I64`, `Utf8`, etc.).
7. Zero-page column: iterator yields zero items.
8. Caller can abandon the iterator mid-stream; the reader
   remains usable for subsequent `read_column` calls.
9. Reader's file cursor isn't exposed publicly; the
   iterator holds a &mut reference so no concurrent
   `read_column` is possible mid-iteration (compile-time
   enforced).
10. Clippy clean.

## 4. Alternatives considered

**A. Owning iterator (not borrowing reader).** Would
require duplicating the reader state. Rejected — borrowing
is the natural fit.

**B. Async iterator.** Deferred; no async in the current
tree.

**C. Expose `read_page` as pub.** Callers could roll their
own iteration. Rejected — too low-level; the ColumnMeta
dance would leak internals.

## 5. Open design questions

None substantive.

## 6. Acceptance evidence

1. All existing 1096 workspace tests pass.
2. New §3.5 tests added, all passing.
3. Clippy clean.
