# ADR 0082 — `.derc` multi-page `DictIndex32` columns (implementation; **ACCEPTED**)

**Status: ACCEPTED on 2026-11-15 and implemented.** See the [verification doc](../implementation/BRADIAN_BACE_DERC_MULTI_PAGE_DICT_VERIFICATION_2026_11_15.md).

Closes the ADR 0081 §2 non-goal. The
multi-page writer now splits plain and UTF-8 columns, but
ADR 0081 §2 explicitly deferred `DictIndex32`: `ColumnBuffer::slice`
panics on dict columns and `concat_buffers` returns
`Unsupported`. Operators who want multi-page segments for
small-page dict-friendly data (the DHIS2 UID / Period columns
that default to dict encoding) today must either disable dict
encoding (threshold=0.0) or accept single-page dict columns.

This ADR closes that gap with a **conservative dict-column
split** design: each page carries a complete copy of the
column-level dict, and pages concat cleanly because they all
share the same dict bytes. The alternative (per-page
sub-dicts with on-read union) is rejected in §4 as more
complexity than ADR 0082 should carry.

Owner review required before any code lands per CLAUDE.md §4.9.

## 1. Context

- `.derc`'s `DictIndex32` buffer has `{ indices, validity,
  dict_offsets, dict_data }`. The dict is a Utf8 blob keyed
  by an offsets vector; `indices[i]` names one dict entry.
- `ColumnBuilder::new_dict(logical_type)` builds the whole
  column as a single dict (one `(dict_offsets, dict_data)`
  pair for the entire column). This is already a
  "column-level dict" conceptually, but the current writer
  emits it as one page.
- ADR 0081 §2 non-goal: multi-page `DictIndex32` is
  unsupported. `ColumnBuffer::slice` panics; `concat_buffers`
  returns `Unsupported`; the writer calls
  `materialize_dict_to_utf8` to force materialization when
  the chosen inner codec is `PlainUtf8` but the buffer is
  `DictIndex32`.
- The dict-encoded Utf8-family columns (default for
  `LogicalType::{Utf8, Uid, Period}`) are precisely the
  columns most likely to benefit from `.derc` multi-page
  writes with BACE dictionary compression (ADRs 0078, 0079,
  0081). Fixing this unblocks the most natural workload for
  the BACE dict-mode story.

## 2. Scope of this ADR

**In scope:**

- `ColumnBuffer::slice` for `DictIndex32` returns a
  `DictIndex32` with:
  - `indices` = sliced from the input
  - `validity` = sliced
  - `dict_offsets` + `dict_data` = **cloned verbatim** from
    the input (the full column dict is duplicated into
    every page)
- Writer: dict-encoded columns no longer bypass the split;
  they split like any other non-dict column. Every emitted
  page carries the full column dict in its payload. The
  per-page `PageCodec::Dictionary` encoder produces the
  familiar dict-encoded bytes.
- Reader `concat_buffers` for `DictIndex32`:
  - If all pages' dict bytes are byte-identical (the common
    case the writer produces), return a `DictIndex32` with
    the shared dict + concatenated indices.
  - If pages have differing dicts (crafted / future-writer
    case), fall back to Utf8 materialization and return a
    `ColumnBuffer::Utf8` instead.
- `ColumnBuilder::new_dict` + related builder path unchanged.
- The writer's `materialize_dict_to_utf8` branch is removed
  (or restricted to the `PlainUtf8` inner-codec case where
  it still applies). Dict columns no longer force
  materialization.
- Tests covering §3.5:
  - Multi-page dict column round-trips to byte-identical
    strings.
  - All pages carry the same dict bytes (writer invariant).
  - Multi-page BACE/BACA/dict trailers work per-page on
    dict columns too.
  - Zone-map merge preserves correct per-column counts.
  - Edge: reader observes differing dicts → produces Utf8
    output (type promotion).
- **Honest size tradeoff documented in §10.** Duplicating
  the dict into every page costs storage; the §3.5 item 8
  test records observed bytes vs ADR 0081's PlainUtf8
  equivalent for the same workload.

**Explicitly out of scope** (future ADRs):

- **Column-level shared-dict format.** Would require a
  format bump (dict stored once at column level, pages
  reference it). This ADR sticks with per-page dict
  duplication to avoid the format change and the
  back-compat obligation.
- **Dict-union on concat** (merging differing per-page
  dicts into one). The writer never produces this shape in
  v1 — all pages share the column dict — so the on-read
  "Utf8 fallback" is sufficient. Dict-union is deferred to
  the sibling ADR that would also do a format bump.
- **Per-page sub-dicts** optimized to only contain entries
  the page actually references. Would save per-page bytes
  but requires index remapping. Deferred.
- **Streaming dict-page reads.** Still materialize the
  whole column (same as ADR 0081).
- **CLI exposure.** No new flags.
- **Changes to BACE/BACA/dict envelope semantics.** All
  stay per-page.

## 3. Proposed design

### 3.1 `ColumnBuffer::slice` for `DictIndex32`

```rust
ColumnBuffer::DictIndex32 { indices, validity, dict_offsets, dict_data } => {
    ColumnBuffer::DictIndex32 {
        indices: indices[start..end].to_vec(),
        validity: validity.slice(start, end),
        dict_offsets: dict_offsets.clone(),
        dict_data: dict_data.clone(),
    }
}
```

No index rebasing — the sliced indices still name the same
dict entries. Dict bytes duplicated per page (the storage
tradeoff documented in §10).

### 3.2 Writer dict-column split

`write_column`'s current special case for
`PageCodec::Dictionary` (single-page bypass via the
`is_dict` flag) is removed. The split loop runs for all
columns regardless of codec; dict-encoded columns produce
N pages each carrying the full dict.

`materialize_dict_to_utf8` stays for the case where the
inner codec is `PlainUtf8` but the buffer is `DictIndex32`
(threshold=0.0 explicit override). That's unchanged.

### 3.3 Reader `concat_buffers` for `DictIndex32`

```rust
PhysicalType::DictIndex32 => {
    // Check dict compatibility across pages.
    let first_dict = extract_dict(&pages[0])?;
    let all_same = pages.iter().skip(1)
        .all(|p| dict_matches(p, first_dict));
    if all_same {
        // Fast path: concat indices directly; share the dict.
        let mut indices = Vec::new();
        let mut validity = Bitmap::with_capacity(total);
        for page in &pages {
            let ColumnBuffer::DictIndex32 { indices: idx, validity: val, .. } = page
                else { unreachable!() };
            indices.extend_from_slice(idx);
            validity.extend_from(val);
        }
        Ok(ColumnBuffer::DictIndex32 {
            indices, validity,
            dict_offsets: first_dict.offsets.clone(),
            dict_data: first_dict.data.clone(),
        })
    } else {
        // Fallback: materialize each page's indices to Utf8,
        // then concat as Utf8. Type-promotion on concat.
        let utf8_pages: Vec<_> = pages.into_iter()
            .map(materialize_dict_to_utf8_owned)
            .collect();
        concat_buffers_utf8(utf8_pages)
    }
}
```

The fallback path is reachable only if the writer produced
pages with differing dicts — something no v1 writer does.
It exists for forward compatibility with a future writer
that might do per-page sub-dict optimization.

### 3.4 `materialize_dict_to_utf8` stays but used differently

The helper in `writer.rs` becomes used ONLY for the
PlainUtf8 inner-codec override case (threshold=0.0 or high
cardinality). Dict columns use the new slice path instead.

The equivalent reader-side helper for the concat fallback
is a straightforward clone of the writer-side function.

### 3.5 Compliance obligations (implementation test checklist)

1. Default options (no `max_rows_per_page`) → byte-identical
   output for every pre-ADR-0082 test. Dict columns still
   emit one page per column when the split isn't enabled.
2. `max_rows_per_page = Some(50)` on a 200-row dict-encoded
   column emits 4 pages, each carrying the full column dict
   in its payload.
3. Multi-page dict column round-trips to byte-identical
   strings (every row's value matches the input).
4. All pages of a multi-page dict column carry the same
   dict bytes (`page.payload_len` for the dict portion is
   equal across pages; the on-disk dict blob is
   byte-identical).
5. Concat of pages with identical dicts returns
   `DictIndex32` (fast path), NOT `Utf8` (type
   preservation).
6. Concat of pages with differing dicts returns `Utf8`
   (type-promoted fallback). Simulated by hand-crafting a
   segment with two pages where page 2's dict has an extra
   entry.
7. Multi-page dict column under BACE wiring (ADR 0076)
   round-trips correctly — each page's BACE envelope wraps
   its own dict-encoded body plus the duplicated dict
   bytes.
8. **Honest size test:** on a workload where plain-UTF-8
   multi-page beats dict-encoded multi-page (because the
   per-page dict duplication cost outweighs the index
   compactness), record observed bytes. The claim this ADR
   makes is NOT "dict multi-page is always smaller" — it
   is "dict multi-page is now reachable and round-trips
   correctly; operators choose the shape that fits their
   workload."
9. BACA (ADR 0080) on multi-page dict column round-trips
   under matching key; wrong-key fails closed per-page
   unchanged.
10. BACE dictionary scope (ADR 0079 §3.3) on multi-page
    dict column: the pool entry covers the writer's dict
    output, and the reader decodes correctly across pages.

## 4. Alternatives considered

**A. Column-level shared dict format.** Store the dict
once at column level (in the footer or a sidecar region),
pages reference it by relative offset. Pros: zero
duplication cost. Cons: requires format bump from v1.3 →
v1.4 with a new feature flag, new on-disk layout, and the
full "readers of earlier versions reject" obligation.
Deferred to a separate ADR.

**B. Dict-union on concat.** When pages have differing
dicts, build a union dict and remap indices. Pros:
preserves `DictIndex32` output even with per-page sub-dicts.
Cons: index remap is non-trivial; no writer today produces
the shape that would need it. Deferred.

**C. Per-page sub-dict optimization.** Writer prunes the
column dict to only entries each page actually uses.
Smaller per-page dict bytes but needs index remap at write
time. Deferred.

**D. Keep the ADR 0081 §2 non-goal (don't ship).** The
`materialize_dict_to_utf8` workaround is sufficient for
operators who want multi-page dict-friendly columns.
Rejected: this workaround forces non-dict-encoded on-disk
storage, defeating the dict codec's compactness benefit
entirely. ADR 0082 keeps the dict benefit at a known
duplication cost.

**E. Only enable multi-page dict when all pages would share
the same dict.** This is what we end up with; it's the
"conservative" design. Formalized as §3.1 — the writer's
per-page split carries the whole column dict so all pages
are identical-dict by construction.

## 5. Open design questions (owner review)

1. **Duplication cost.** Draft: accept per-page dict
   duplication; §3.5 item 8 records the size observation
   honestly. Alternative: defer the whole ADR until a
   format bump addresses column-level shared dict.
   Confirm.
2. **Type promotion on differing-dict concat.** Draft:
   fall back to `Utf8`. Alternative: return `Corruption`
   because no v1 writer produces that shape. Confirm the
   forward-compat stance (Utf8 fallback).
3. **Writer removal of `is_dict` bypass.** Draft: the ADR
   0081 §3.2 one-page bypass for dict columns is removed.
   Alternative: keep it as an operator opt-in (e.g.,
   `max_rows_per_page_for_dict`). Confirm removal.
4. **`materialize_dict_to_utf8` writer-side purpose.**
   Draft: stays for the PlainUtf8-codec-with-DictIndex32-
   buffer case; no longer called for dict columns. Confirm.
5. **Zone-map merge on dict columns.** Draft: no change
   from ADR 0081 §3.3 — dict columns use the same per-page
   `compute_zone_map` and column-level merge. Confirm.
6. **BACE dictionary scope interaction.** Draft: when both
   ADR 0079 dict and ADR 0082 multi-page dict column are in
   play, the writer emits a v1.2 segment with the BACE dict
   pool entry for the column + the column's own
   per-page-duplicated dict inside each page. Double dict
   layer is possible (BACE-level + inner Dictionary codec).
   Confirm.
7. **ADR 0081 §3.5 item 10 preservation.** Draft: unchanged.
   The test uses non-dict Utf8 (threshold=0.0); ADR 0082
   doesn't touch that code path. Confirm.

## 6. Impact on invariants

- **INV-001/002/003/004:** N/A — `.derc` is a durable
  analytical format.
- **INV-005 recoverable lifecycle:** additive. Default
  behavior unchanged. Opt-in multi-page dict output is
  readable by the same v1.0/v1.1/v1.2/v1.3 reader rules
  because no new feature flag is added.

## 7. Dependencies this ADR would add

None.

## 8. What this ADR explicitly does NOT do

- Change the on-disk `.derc` format (no new minor version,
  no new feature flag, no new page shape).
- Introduce column-level shared dict.
- Implement dict-union or per-page sub-dict optimization.
- Add streaming reads.
- Add CLI exposure.
- Change BACE/BACA/dict envelope semantics.

## 9. Acceptance evidence required before marking Accepted

1. ADRs 0060–0081 accepted — **satisfied**.
2. Owner sign-off on §5 open design questions, especially
   Q1 (duplication cost accepted) and Q3 (writer bypass
   removal).
3. Implementation satisfying every item in §3.5.
4. §3.5 item 8 **recorded honestly** — a dict-multi-page
   size that's not a strict win vs plain-Utf8-multi-page is
   acceptable; the claim this ADR makes is reachability +
   correctness, not a new size win beyond ADR 0081's.
5. No regressions in any dap-storage, bradian-relational,
   or workspace test.

Only then can this ADR move from **PROPOSED** to **ACCEPTED**.

## 10. Duplication-cost observation framework

The per-page-duplicated dict means an N-page dict column
stores N copies of the dict bytes on disk. For a column
whose dict is D bytes and segment has P pages:

- Per-page cost: D + (indices payload) + (envelope)
- Total dict cost: N × D (dict only)

For comparison, single-page dict stores D once. The ADR
0081 §3.5 item 10 size win depends on BACE dict-mode
compressing the duplicate dict bytes across pages well.

**The §3.5 item 8 test records the observed numbers
honestly** — no strict-smaller assertion against a
hypothetical column-level-shared-dict baseline, because no
such baseline exists until a future ADR implements it.
