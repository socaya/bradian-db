# ADR 0081 — `.derc` multi-page column writer (implementation; **PROPOSED**)

**Status: PROPOSED — not accepted for implementation.**

Blocked on the accepted BACE workstream through ADR 0080. This
ADR finishes the single remaining v1 limitation that prevents
ADR 0079's dictionary pool from paying for itself: the `.derc`
writer currently emits exactly **one page per column** regardless
of row count. With the reader's `concat_buffers` still returning
"unsupported", multi-page is genuinely not reachable.

This ADR adds a bounded multi-page writer path, implements
`concat_buffers` for every `ColumnBuffer` variant that supports
it, and verifies that ADR 0079's dictionary pool **actually
saves bytes** when amortized across multiple pages on a realistic
row count.

Owner review required before any code lands per CLAUDE.md §4.9.

## 1. Context

- `.derc` format v1 was designed with multi-page support
  (`ColumnMeta.pages: Vec<PageMeta>`), but the writer only
  emits one page per column.
- `SegmentReader::read_column` handles `pages.len() == 1`
  directly and routes `> 1` through `concat_buffers`, which
  returns `DapError::unsupported("multi-page column
  concatenation not implemented in v1")`.
- Every ADR in the BACE workstream that touches `.derc`
  (0076, 0079, 0080) has been constrained by "single page
  per column." The ADR 0079 §10 size-reduction test had to
  relax from "strictly smaller" to "pool-overhead-bounded"
  because the dict pool couldn't amortize.
- Fixing this is bounded: the format already supports it;
  only the writer's split logic and the reader's concat
  logic need to land.

## 2. Scope of this ADR

**In scope:**

- `SegmentWriterOptions.max_rows_per_page: Option<usize>`.
  Default `None` preserves today's one-page-per-column
  shape. When `Some(n)`, columns are split into ceil(rows /
  n) pages of at most `n` rows each.
- Writer: `write_column` emits N `PageMeta` entries per
  column when the option is set.
- Column-level zone-map `merge` across pages
  (`ZoneMap::merge(&self, &other) -> ZoneMap`).
- Reader: `concat_buffers` implemented for every
  `ColumnBuffer` variant **except** `DictIndex32`. The
  dict variant's per-page dictionaries can't be merged
  without either rebuilding or dict-union semantics; this
  ADR explicitly keeps dict-encoded columns single-page
  by forcing the writer to emit one page for dict columns
  even when `max_rows_per_page` is set.
- Validity bitmap concat with a `Bitmap::extend` helper.
- UTF-8 concat: offsets adjusted + byte buffer concatenated.
- BACE / BACA / dict trailers continue to apply **per
  page** — each page carries its own envelope.
- Size-reduction demonstration test that puts ADR 0079's
  "strict smaller" claim to the test on a multi-page
  segment: the `v1.2 ≤ v1.1 + pool_cost_bound` relaxation
  from ADR 0079 §10 is replaced with a **strict `v1.2 <
  v1.1`** assertion.
- Compliance tests for primitive-type concat (Bool, I32,
  I64, F64, Decimal128, Timestamp, Utf8) with nullable
  and non-nullable columns.

**Explicitly out of scope** (future ADRs):

- **Multi-page `DictIndex32` columns.** The writer refuses
  to split dict-encoded columns in this ADR. A future ADR
  can implement dict-union or per-page-dict-with-column-
  dict-pool semantics.
- **Per-column `max_rows_per_page` overrides.** The option
  is a global row count applied uniformly; column-specific
  thresholds are deferred.
- **Row-group concept.** `.derc` v1 has one implicit row
  group per segment; adding multiple row groups changes
  footer shape. Deferred.
- **Streaming reads.** The reader still materializes the
  whole column into a single `ColumnBuffer`; page-at-a-
  time iteration is a separate API.
- **Auto-sizing** of `max_rows_per_page` based on memory
  budget or predicted compression. This ADR takes a
  concrete row count; auto-tuning is future work.
- **BACE on multi-page per-page MAC sharing.** Each page
  still has its own BACE/BACA envelope and its own MAC
  (ADR 0080 semantics unchanged).
- **CLI exposure.** `bradian-db` gains no new flags; the
  option is library-only.

## 3. Proposed design

### 3.1 Writer option

```rust
pub struct SegmentWriterOptions {
    // ... existing fields ...
    /// ADR 0081 — if `Some(n)`, each column (except
    /// dict-encoded) is split into pages of at most `n`
    /// rows. `None` (default) emits one page per column
    /// matching all prior behavior.
    pub max_rows_per_page: Option<usize>,
}
```

Defaulted to `None` in `impl Default`. Every prior test
still produces identical bytes.

### 3.2 Writer `write_column` split logic

Pseudo-code:

```rust
let page_size = match (dict_encoded, self.options.max_rows_per_page) {
    (true, _) => n_rows,            // dict columns stay single-page
    (false, Some(limit)) => limit,
    (false, None)        => n_rows, // single-page preserves ADR 0076 behavior
};

let mut pages = Vec::new();
let mut merged_zone = ZoneMap::empty();
for page_rows in (0..n_rows).step_by(page_size) {
    let slice = buf.slice(page_rows..min(page_rows + page_size, n_rows));
    let (page_meta, page_zone) = self.write_one_page(col, slice, codec)?;
    merged_zone = merged_zone.merge(&page_zone);
    pages.push(page_meta);
}
Ok((pages, merged_zone))
```

`buf.slice(range)` is a new method on `ColumnBuffer` that
returns a borrowed-or-cloned sub-slice for all variants
except `DictIndex32`. For `DictIndex32` the writer bypasses
slicing by emitting the whole buffer as a single page.

### 3.3 `ZoneMap::merge`

```rust
impl ZoneMap {
    pub fn merge(&self, other: &ZoneMap) -> ZoneMap {
        ZoneMap {
            row_count: self.row_count + other.row_count,
            null_count: self.null_count + other.null_count,
            min: match (&self.min, &other.min) {
                (None, None) => None,
                (Some(a), None) => Some(a.clone()),
                (None, Some(b)) => Some(b.clone()),
                (Some(a), Some(b)) => Some(if a <= b { a.clone() } else { b.clone() }),
            },
            max: /* dual logic */,
        }
    }
    pub fn empty() -> Self { ZoneMap { row_count: 0, null_count: 0, min: None, max: None } }
}
```

Lexicographic `Vec<u8>` comparison for min/max matches the
zone-map encoding used by the single-page path today.

### 3.4 Reader `concat_buffers`

```rust
fn concat_buffers(col: &ColumnMeta, pages: Vec<ColumnBuffer>) -> DapResult<ColumnBuffer> {
    match col.physical_type {
        PhysicalType::I32 => concat_primitive_i32(pages),
        PhysicalType::I64 => concat_primitive_i64(pages),
        // ... one helper per variant ...
        PhysicalType::DictIndex32 => Err(DapError::unsupported(
            "multi-page dictionary columns not supported in ADR 0081; dict columns stay single-page",
        )),
        _ => ...
    }
}
```

Each variant's helper:
- Preallocates a `Vec` with the summed capacity.
- Extends `data` and `validity` from each page.
- UTF-8: adjusts offsets by `prev.data.len()` before extending.

### 3.5 Compliance obligations (implementation test checklist)

1. Default options (no `max_rows_per_page`) → byte-
   identical v1.0/v1.1/v1.2/v1.3 output for every
   previously-covered scenario.
2. `max_rows_per_page = Some(100)` on a 350-row segment
   emits 4 pages per non-dict column (100+100+100+50).
3. Round-trip of multi-page segment yields the same
   logical column values as a single-page segment on the
   same rows.
4. `ZoneMap::merge` row/null counts add correctly; min/max
   take lexicographic bounds.
5. Validity bitmaps across pages reproduce the correct
   NULL mask after concat.
6. UTF-8 column: offsets after concat point correctly into
   the merged byte buffer.
7. Dict-encoded columns emit one page even when
   `max_rows_per_page` is set (writer bypass).
8. Reading a dict column from a multi-page writer still
   succeeds (because the writer never actually emitted
   multi-page dict).
9. BACE envelope flag (ADR 0076), dict pool (ADR 0079),
   and BACA auth (ADR 0080) all work **per page**: each
   page's trailer carries its own tag; the reader
   dispatches per page.
10. **Strict size-reduction demonstration.** The ADR
    0079 §10 observation ("v1.2 is 266 bytes LARGER than
    v1.1 on single-page redundant data") is replaced on
    multi-page input: a 4096-row segment split into 32
    pages of 128 rows each shows `v1.2 < v1.1` strictly,
    unlocking the dict-amortization win ADR 0079 couldn't
    reach alone.

## 4. Alternatives considered

**A. Add streaming page-at-a-time reads.** Rejected —
different API shape (iterator vs ColumnBuffer); multi-
page concat is strictly simpler and already matches the
existing reader contract. Streaming can be a separate ADR.

**B. Implement dict-union across pages.** Would allow
multi-page dict columns. Rejected: significant complexity
(dict merging, offset adjustment, potentially worse
compression). Deferred to a future ADR.

**C. Auto-pick `max_rows_per_page` based on memory
budget.** Rejected: explicit row count is honest and
deterministic for v1.

**D. Split pages by byte count instead of row count.**
Rejected: byte-count split would be adaptive and nicer
for mixed-row-size workloads, but requires a two-pass
writer. Row count is simpler.

**E. Introduce multi-row-group semantics.** Rejected:
format change beyond what's needed. v1 row groups stay
one-per-segment; multi-page within a row group is enough.

## 5. Open design questions (owner review)

1. **`max_rows_per_page` default.** Draft: `None`
   (unchanged behavior). Alternatives: `Some(10_000)` as a
   production default. Confirm the conservative `None`.
2. **Dict columns bypass vs error on `max_rows_per_page`.**
   Draft: bypass (dict column emits single page even when
   option set). Alternative: error. Confirm bypass.
3. **`ZoneMap::empty()` placement.** Draft: public helper.
   Confirm.
4. **UTF-8 offset layout.** Draft: offsets are `Vec<u32>`
   relative to the page's `data` buffer; after concat they
   are rebased to the merged buffer. Confirm.
5. **§3.5 item 10 strict assertion.** Draft: multi-page
   v1.2 < v1.1 strictly on a 4096-row / 32-page segment.
   Confirm as a hard-fail acceptance gate (matches ADR
   0077 pattern).
6. **Validity bitmap concat algorithm.** Draft: byte-
   aligned when lengths are byte-multiples; bit-shift
   otherwise. Confirm.
7. **`ColumnBuffer::slice`.** Draft: new method on all
   non-dict variants; returns owned `ColumnBuffer` with
   data/validity sub-ranges. Confirm.

## 6. Impact on invariants

- **INV-001/002/003/004:** N/A — `.derc` is a durable
  analytical format.
- **INV-005 recoverable lifecycle:** additive. Default
  behavior is unchanged. Opt-in multi-page writes stay
  readable via the same `.derc` v1.0/v1.1/v1.2/v1.3 format
  rules; no feature flag is added because multi-page was
  always in the format, only unreachable.

## 7. Dependencies this ADR would add

None.

## 8. What this ADR explicitly does NOT do

- Multi-page `DictIndex32` columns.
- Per-column row-threshold overrides.
- Streaming reads.
- Auto-sized page threshold.
- Multi-row-group segments.
- CLI exposure.
- Changes to BACE/BACA/dict envelope semantics (all stay
  per-page).

## 9. Acceptance evidence required before marking Accepted

1. ADRs 0060–0080 accepted — **satisfied**.
2. Owner sign-off on §5 open design questions, especially
   Q1 (default `None`) and Q5 (strict multi-page size test).
3. Implementation satisfying every item in §3.5.
4. **§3.5 item 10 PASSES** (`v1.2 < v1.1` strict on
   multi-page redundant payload). If it fails, this ADR
   should be reconsidered.
5. No regressions in any existing `dap-storage`,
   `bradian-relational`, or workspace test.

Only then can this ADR move from **PROPOSED** to **ACCEPTED**.
