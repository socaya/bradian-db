# ADR 0083 — `.derc` column-level shared dictionary format (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-15 and implemented.** See the [verification doc](../implementation/BRADIAN_BACE_DERC_COLUMN_DICT_VERIFICATION_2026_11_15.md).

Follow-on to ADR 0082. The accepted ADR 0082 shape duplicates
the column dict into every page (per-page copy, byte-identical
by writer construction), which costs N × dict_bytes on disk.
For high-cardinality dicts × many-page columns this duplication
overhead dominates. ADR 0082 §4 alternative (A) was explicitly
deferred for a format bump — this ADR does that bump.

Owner review required before any code lands per CLAUDE.md §4.9.

## 1. Context

- `.derc` v1.0–v1.3 pack `(indices, dict_offsets, dict_data)`
  inside every `PageCodec::Dictionary` page payload. ADR 0082
  made N-page dict columns work by cloning the dict into
  every page at write time (fast-path concat at read time).
- Per-page dict duplication cost = `N × size_of(dict_offsets +
  dict_data)`. For a 200-row × 50-rows/page = 4-page column
  with a 4-entry dict (~40 bytes), that's +120 bytes. For a
  4096-row × 8-rows/page = 512-page column with a 200-entry
  UID dict (~8 KB), that's +4 MB of redundant bytes —
  practically defeating the dict codec's purpose.
- Readers of v1.3 and earlier still need to reject segments
  that set the new column-dict feature flag, per the existing
  feature-flag superset convention (`ALL_SUPPORTED_V1_N`).

## 2. Scope of this ADR

**In scope:**

- A new `.derc` minor version `V1_4` and feature flag
  `COLUMN_DICT` (bit 8) covering the new on-disk shape.
- A new column-level dict region between the last page region
  and the footer. For each column that opts into column-dict
  mode, one record stores `(dict_offsets, dict_data)` with its
  CRC32. The footer gains per-column optional
  `column_dict_offset` / `column_dict_bytes` / `column_dict_crc32`
  fields; absence = legacy per-page shape.
- When column-dict mode is active for a column, its
  `PageCodec::Dictionary` pages store only `(indices, validity)`.
  The reader loads the single column dict once; every page's
  indices reference it.
- Writer opt-in via `SegmentWriterOptions.use_column_dict:
  bool` (default `false` for v1.3 compat). When set and a
  column lands on the Dictionary codec, that column uses the
  v1.4 shape.
- Reader rejects v1.4 segments on the legacy `SegmentReader::
  open` path and on `open_with_bace` when the policy doesn't
  include v1.4 in its supported set.
- Interaction with existing feature flags: `COLUMN_DICT` is
  orthogonal to `COMPRESSED_PAGES`, `BACE_PAGE_ENVELOPE`,
  `BACE_PAGE_DICTIONARY`, `BACE_PAGE_AUTHENTICATED`. Any
  combination is legal; per-page envelope dispatch is
  unchanged.
- `ColumnBuffer::slice` on `DictIndex32`: unchanged (the
  v1.4 writer builds page buffers with `dict_offsets` /
  `dict_data` set to empty vectors so slice still works — or
  the writer passes the column dict in via a new path; §3
  settles this).
- Reader `concat_buffers` for `DictIndex32`: fast path
  unchanged (all pages share the same dict). The column-level
  dict is attached to each page's buffer before concat so the
  existing fast-path identity check matches.
- Tests covering §3.5 (see below).

**Explicitly out of scope** (future ADRs):

- **Removing the v1.3 per-page shape.** v1.3 pages stay
  readable indefinitely; the two shapes coexist.
- **Shared dict across multiple columns.** Each column still
  has its own dict region; cross-column dict sharing is a
  later ADR.
- **Online migration of v1.3 segments to v1.4.** Offline
  re-write only.
- **MAC coverage of the column-level dict.** When ADR 0080
  BACA is active, the column-dict region is NOT included in
  any page's MAC. It carries its own CRC32. A future ADR
  extending BACA to cover column-dict is a separate design.
- **CLI exposure.** No new flags yet.
- **Changes to BACE page-envelope or BACA MAC semantics.**
  Unchanged per-page.

## 3. Proposed design

### 3.1 Format version and feature flag

```rust
// crates/dap-storage/src/segment/format.rs
pub const SEGMENT_VERSION_MINOR_V1_4: u16 = 4;

pub const COLUMN_DICT: u64 = 1 << 8;
pub const ALL_SUPPORTED_V1_4: u64 =
    ALL_SUPPORTED_V1_3 | COLUMN_DICT;

pub fn is_subset_supported_v1_4(flags: u64) -> bool {
    flags & !ALL_SUPPORTED_V1_4 == 0
}
```

### 3.2 On-disk layout

```
[header (64 bytes)]
[page regions ... per column, per page]
[column-dict region]        ← NEW, optional per column
    for each column with column-dict:
        [u32 dict_offsets_len]
        [u32 dict_data_len]
        [dict_offsets bytes]
        [dict_data bytes]
        [u32 crc32 of the two blobs concatenated]
[footer]
    for each ColumnMeta:
        existing fields ...
        [u8 has_column_dict]            ← NEW
        if has_column_dict:
            [u64 column_dict_offset]    ← NEW
            [u64 column_dict_bytes]     ← NEW
            [u32 column_dict_crc32]     ← NEW
```

The column-dict region is written AFTER all page regions and
BEFORE the footer. The footer's per-column fields point at
absolute offsets within the file.

### 3.3 Writer path

1. `write_column` detects `codec == PageCodec::Dictionary`
   AND `self.options.use_column_dict`. If both hold, enters
   the v1.4 path.
2. The column's `DictIndex32` buffer provides the column
   dict once. The writer records
   `(dict_offsets.clone(), dict_data.clone())` in a pending
   `ColumnDictRegion` keyed by column name.
3. For each page, `ColumnBuffer::slice` runs as today, but
   `emit_one_page` for the Dictionary codec serializes ONLY
   `(indices, validity)` into the page payload — NOT the dict
   bytes. A new on-disk page encoding variant distinguishes
   v1.3-shape (`dict_offsets` + `dict_data` inside the page)
   from v1.4-shape (indices only).
4. After all pages are written, `write_column_dict_region`
   emits the stored dicts in column order and records their
   absolute offsets.
5. The footer serialization includes the
   `has_column_dict + offset/bytes/crc32` tuple per column.

### 3.4 Reader path

1. `read_footer_with_features` parses the new per-column
   optional fields. If any column has `has_column_dict`, the
   segment must set `COLUMN_DICT` in its flags; otherwise
   `Corruption`.
2. On `read_column`, if the column has a `column_dict_offset`,
   the reader loads the column dict region once and verifies
   its CRC32. Hard-fail on mismatch.
3. For each page: the reader decodes the page payload as
   `(indices, validity)` only. Before returning the page's
   `ColumnBuffer::DictIndex32`, the reader attaches the
   column-level `dict_offsets` + `dict_data` by cloning (or
   by reference-counted sharing — §5 Q4 settles).
4. `concat_buffers` fast-path identity check matches by
   construction (all pages share the same dict bytes).

### 3.5 Compliance obligations (implementation test checklist)

1. Default `SegmentWriterOptions` (no `use_column_dict`) →
   byte-identical output for every pre-ADR-0083 test. v1.3
   format version stamped; `COLUMN_DICT` flag NOT set.
2. `use_column_dict = true` on a single-page dict column →
   v1.4 segment, `COLUMN_DICT` flag set, column-dict region
   present, page payload does NOT contain the dict bytes.
3. `use_column_dict = true` on a 200-row × 50-rows/page = 4-
   page dict column → one column-dict region, four pages
   each carrying only `(indices, validity)`.
4. Round-trip: every row's value matches the input.
5. Fast-path concat returns `DictIndex32` (the reader
   attaches the shared dict before concat).
6. CRC32 mismatch on the column-dict region → hard-fail with
   a corruption error. The pages are NOT trusted without a
   valid dict.
7. `SegmentReader::open` (legacy path, no BACE wiring) on a
   v1.4 segment with `COLUMN_DICT` set → rejected with
   unsupported-feature error.
8. `SegmentReader::open_with_bace` on a v1.4 segment with
   `COLUMN_DICT` set → accepted and round-trips.
9. BACE/BACA/dict-pool envelopes still apply per-page on v1.4
   (the column-dict region is outside the per-page
   envelope). Round-trip with each envelope variant.
10. **HARD-FAIL strict size test.** On a 4096-row × 8-rows/
    page × 200-entry-dict column workload, v1.4
    (`use_column_dict=true`) must be strictly SMALLER than
    v1.3 (`use_column_dict=false`, ADR 0082 per-page copy).
    Failure to shrink → the ADR's central claim is wrong
    and the design must be revisited.

## 4. Alternatives considered

**A. Dict stored in the footer.** Puts variable-sized blob
inside the fixed-ish footer region. Rejected: inflates the
footer parser complexity and makes seek-to-column-dict
irregular. The §3.2 "between pages and footer" region keeps
the footer small and the dict region seekable via its own
offset.

**B. Dict stored in the first page of the column, subsequent
pages reference it.** Attractive: no new region. Rejected:
cross-page dependency complicates per-page BACE/BACA
envelopes (the "first page" would need special treatment),
and random-page reads require loading page 0 first — ties
the reader to a specific access order.

**C. Shared dict across all columns.** Rejected for this
ADR: deep cross-column coupling, breaks per-column
independence, and a workload with M columns × 1 dict can't
accommodate different schemas per column. Deferred to a
separate ADR if it's ever useful.

**D. Reference-counted dict shared across pages in memory.**
Attractive for memory: one `Arc<Dict>` instead of N clones.
Rejected for v1 implementation: the existing `ColumnBuffer`
is `Clone + Debug + PartialEq + Serialize + Deserialize` and
carries owned `Vec<u8>` fields. Introducing `Arc` here is a
larger change than ADR 0083 should carry. §5 Q4 can
reconsider.

**E. Keep per-page duplication; compress across pages with
BACE dict pool.** This is what ADR 0079 already partially
achieves when the dict pool trains on dict-page bytes.
Rejected as insufficient: the duplication is
O(N × dict_bytes) uncompressed and BACE trains at a
column-granular level, not a per-column-dict granular level.
A real fix removes the duplication from the on-disk shape.

## 5. Open design questions (owner review)

1. **CRC32 vs BLAKE3 for the column-dict region.**
   Draft: CRC32, matching the existing per-page CRC32
   convention (pre-BACE). Alternative: BLAKE3 keyed MAC when
   BACA is active, covering the dict region too. Confirm.
2. **Reader hard-fail on CRC32 mismatch.** Draft: yes, hard-
   fail — the pages' indices are meaningless without a
   verified dict. Confirm.
3. **`use_column_dict` default.** Draft: `false` for
   backward compat and to preserve ADR 0082 §10 "no new size
   win claim beyond what ADR 0081 established." Operators
   explicitly opt in. Alternative: auto-enable when
   `max_rows_per_page.is_some()` AND the chosen codec is
   `Dictionary`. Confirm.
4. **In-memory dict sharing (Arc vs Clone).** Draft: Clone
   (consistency with existing `ColumnBuffer` ownership).
   Alternative: `Arc<DictBytes>` wrapper type. Deferred to a
   follow-up ADR to keep this one tightly scoped.
5. **Column-dict region position.** Draft: between pages and
   footer. Alternative: at the END of the file, after the
   footer trailer — enables appending more columns post-hoc.
   Rejected because `.derc` has no append semantics; confirm.
6. **Interaction with BACE dict pool (ADR 0079).** Draft:
   both apply independently. The BACE dict pool trains on
   raw column input bytes and operates at compression time;
   the ADR 0083 column dict is the Dictionary codec's
   lookup table at decode time. Nothing precludes stacking
   both. Confirm.
7. **BACA scope extension.** Draft: NOT in scope. Column-
   dict region has its own CRC32; MAC coverage is deferred
   to a future BACA-extension ADR. If BACA is active, the
   per-page MACs still work against the pages' `(indices,
   validity)` payloads. Confirm the deferral.

## 6. Impact on invariants

- **INV-001/002/003/004:** N/A — `.derc` is a durable
  analytical format.
- **INV-005 recoverable lifecycle:** additive feature flag.
  Default behavior unchanged. v1.4 segments are readable by
  clients that know the `COLUMN_DICT` flag; v1.3-and-earlier
  readers reject them via the existing
  `is_subset_supported_v1_N` gate.

## 7. Dependencies this ADR would add

None.

## 8. What this ADR explicitly does NOT do

- Change the behavior of v1.0–v1.3 segments. They remain
  fully readable and writable.
- Introduce cross-column dict sharing.
- Migrate existing v1.3 segments.
- Extend BACA MAC coverage.
- Add CLI exposure.
- Introduce `Arc<Dict>` or any reference-counted sharing.

## 9. Acceptance evidence required before marking Accepted

1. ADRs 0060–0082 accepted — **satisfied**.
2. Owner sign-off on §5 open design questions, especially Q1
   (CRC32 vs MAC), Q3 (default opt-in), and Q5 (region
   position).
3. Implementation satisfying every item in §3.5.
4. §3.5 item 10 PASSED — v1.4 strictly smaller than v1.3 on
   the specified workload.
5. No regressions in any dap-storage, bradian-relational, or
   workspace test.
6. The v1.3 → v1.4 feature-flag superset check preserved
   (`ALL_SUPPORTED_V1_3` subset of `ALL_SUPPORTED_V1_4`).

Only then can this ADR move from **PROPOSED** to **ACCEPTED**.

## 10. Size-win hypothesis

The central claim: for a column-dict column of size `D` bytes
split across `N` pages, v1.4 stores ~`D + N × O(indices_bytes)`
vs v1.3's `N × (D + indices_bytes) = N × D + N × indices_bytes`.
The v1.4 win is `(N - 1) × D` bytes of saved dict duplication,
minus the fixed footer-field overhead (~24 bytes per column
with column-dict).

For the §3.5 item 10 workload (D ≈ 8 KB, N = 512), the
expected savings are ~4 MB in uncompressed on-disk size —
well above any BACE or ZSTD cross-page dedup that would
otherwise partially recover the duplication.

The test records the OBSERVED numbers honestly, not just the
hypothetical savings. If the measured delta is positive but
smaller than expected (because ZSTD across pages already
dedups some of the duplicate dict bytes in v1.3), the ADR's
claim is still validated — any strictly-smaller result
confirms the format bump is worthwhile for operators who
care about raw bytes.
