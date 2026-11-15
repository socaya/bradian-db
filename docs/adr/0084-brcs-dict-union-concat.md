# ADR 0084 — `.brcs` dict-union concat for differing-dict pages (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-15 and implemented.** See the [verification doc](../implementation/BRADIAN_BACE_DICT_UNION_VERIFICATION_2026_11_15.md).

Follow-on to ADR 0082. The accepted ADR 0082 §3.3 reader concat
path has two branches for `DictIndex32`: a fast path when all
pages share byte-identical dict bytes (today's writer
invariant), and a fallback that materializes to `Utf8` when
pages carry differing dicts. The fallback loses the dict-encoded
output shape entirely.

This ADR replaces that `Utf8` fallback with a **dict-union**
path: merge per-page dicts into one union dict preserving
first-appearance order, remap each page's indices to the union
numbering, and return a `ColumnBuffer::DictIndex32` with the
unioned dict and concatenated remapped indices.

Owner review required before any code lands per CLAUDE.md §4.9.

## 1. Context

- ADR 0082 §3.3 fallback: differing-dict → `Utf8`
  materialization. The dict-encoded output shape is lost.
- No v1 writer today produces differing-dict pages. The ADR
  0082 writer invariant (every page carries a verbatim copy of
  the full column dict) makes the fast path always apply. ADR
  0083 preserves this (indices-only pages all reference the
  single column dict in the new region). So the fallback is
  **unreachable today**.
- A future writer that does per-page sub-dict pruning (ADR
  0082 §4(C) non-goal) would produce differing dicts: each
  page's dict carries only entries that page's indices
  reference, keeping per-page dict bytes small even without
  the ADR 0083 format bump.
- Users calling the reader today don't observe the fallback.
  This ADR is **pure forward-compat plumbing** for a
  per-page-sub-dict writer that would land in a later ADR.

## 2. Scope of this ADR

**In scope:**

- Reader `concat_buffers` for `DictIndex32`: when pages carry
  differing dicts, build a union dict preserving the order of
  first appearance across pages, remap each page's indices
  into the union numbering, and return a `DictIndex32` with
  the union dict and concatenated remapped indices.
- Null sentinel (`u32::MAX`) passes through unchanged.
- The fast path (identical dicts) is unchanged — same code
  path, same output.
- A new public `ColumnBuffer::concat_dict_pages(pages:
  Vec<ColumnBuffer>) -> BradianResult<ColumnBuffer>` helper
  exposed for integration tests. The reader's internal
  `concat_buffers` delegates to this when the physical type
  is `DictIndex32` and pages need union. Public exposure keeps
  tests driven by the real union code path rather than a
  parallel test copy.
- Tests covering §3.5.

**Explicitly out of scope** (future ADRs):

- **Writer changes.** The writer still produces identical-dict
  pages (ADR 0082 §3.1 invariant). No per-page sub-dict
  writer is introduced here.
- **Changes to ADR 0083 v1.4 column-dict format.** Pages
  reference a single column dict; there is no per-page dict
  to union. This ADR applies only to the ADR 0082 per-page-
  dict shape in v1.3 segments.
- **Cross-column dict sharing.**
- **Streaming union.** The union is materialized in full
  before concat.
- **CLI exposure.**
- **Performance tuning.** Union-building is O(total dict
  entries × avg entry length) via `HashMap<Vec<u8>, u32>`.
  Hash-collision costs are not optimized away.

## 3. Proposed design

### 3.1 `ColumnBuffer::concat_dict_pages`

```rust
impl ColumnBuffer {
    pub fn concat_dict_pages(pages: Vec<ColumnBuffer>) -> BradianResult<ColumnBuffer> {
        if pages.is_empty() {
            return Err(BradianError::corruption("concat_dict_pages: no pages"));
        }
        // Every page must be DictIndex32.
        for (i, p) in pages.iter().enumerate() {
            if !matches!(p, ColumnBuffer::DictIndex32 { .. }) {
                return Err(BradianError::corruption(format!(
                    "concat_dict_pages: page {i} is not DictIndex32",
                )));
            }
        }

        // Fast path: all pages share byte-identical dict bytes.
        // Preserve existing ADR 0082 §3.3 behavior.
        if pages_share_identical_dict(&pages) {
            return concat_dict_pages_identical(pages);
        }

        // Slow path: dict-union.
        let mut union_offsets: Vec<u32> = vec![0];
        let mut union_data: Vec<u8> = Vec::new();
        let mut entry_to_idx: std::collections::HashMap<Vec<u8>, u32>
            = std::collections::HashMap::new();
        let mut total_rows = 0usize;
        for p in &pages {
            let ColumnBuffer::DictIndex32 { dict_offsets, dict_data, indices, .. } = p
                else { unreachable!() };
            for (i, _) in indices.iter().enumerate() { let _ = i; }
            for i in 0..dict_offsets.len() - 1 {
                let s = dict_offsets[i] as usize;
                let e = dict_offsets[i + 1] as usize;
                let bytes = &dict_data[s..e];
                if !entry_to_idx.contains_key(bytes) {
                    let new_idx = (union_offsets.len() - 1) as u32;
                    entry_to_idx.insert(bytes.to_vec(), new_idx);
                    union_data.extend_from_slice(bytes);
                    union_offsets.push(union_data.len() as u32);
                }
            }
            total_rows += indices.len();
        }

        let mut out_indices: Vec<u32> = Vec::with_capacity(total_rows);
        let mut out_validity = Bitmap::with_capacity(total_rows);
        for p in &pages {
            let ColumnBuffer::DictIndex32 {
                dict_offsets, dict_data, indices, validity,
            } = p else { unreachable!() };
            for (row, &old_idx) in indices.iter().enumerate() {
                if old_idx == u32::MAX {
                    out_indices.push(u32::MAX);
                    out_validity.push(validity.is_valid(row));
                    continue;
                }
                let s = dict_offsets[old_idx as usize] as usize;
                let e = dict_offsets[old_idx as usize + 1] as usize;
                let bytes = &dict_data[s..e];
                let new_idx = *entry_to_idx.get(bytes).expect(
                    "union built from exactly these bytes",
                );
                out_indices.push(new_idx);
                out_validity.push(validity.is_valid(row));
            }
        }
        Ok(ColumnBuffer::DictIndex32 {
            indices: out_indices,
            validity: out_validity,
            dict_offsets: union_offsets,
            dict_data: union_data,
        })
    }
}
```

### 3.2 Reader delegation

`reader.rs::concat_buffers` for `DictIndex32`:

```rust
PhysicalType::DictIndex32 => {
    ColumnBuffer::concat_dict_pages(pages)
}
```

The existing identical-dict fast path moves inside
`concat_dict_pages` (§3.1). The existing Utf8 fallback branch
is **removed** — dict-union now covers that case while
preserving the output shape.

### 3.3 Null handling

`u32::MAX` is the null sentinel (set by
`DictUtf8Builder::append_null`). It never references a dict
entry; the union-building loop skips it when iterating
indices, and the remapping loop passes it through unchanged.
Validity bits concat per page in row order.

### 3.4 Determinism

Union order = order of first appearance across pages, pages
walked in input order, entries within a page walked in
dict-offset order. This yields a stable byte-identical output
for any fixed input, which matters for golden-byte tests of
a future per-page-sub-dict writer that might depend on the
union shape.

### 3.5 Compliance obligations (implementation test checklist)

1. All existing ADR 0082 §3.5 tests (`multi_page_dict.rs`
   items 1–10) pass unchanged. The identical-dict fast path
   produces the same `DictIndex32` output as today.
2. Hand-crafted two-page `DictIndex32` input with
   **identical** dicts → `concat_dict_pages` returns a
   `DictIndex32` with the shared dict preserved (fast path).
3. Hand-crafted two-page input with **non-overlapping**
   dicts (page 1 has `["a", "b"]`, page 2 has `["c", "d"]`)
   → returns a `DictIndex32` with union dict `["a", "b", "c",
   "d"]` and correctly remapped indices.
4. Hand-crafted two-page input with **partially overlapping**
   dicts (page 1 has `["a", "b"]`, page 2 has `["b", "c"]`)
   → returns union dict `["a", "b", "c"]` (b dedup'd); page
   2's indices to "b" map to union index 1, "c" to 2.
5. Null sentinel (`u32::MAX`) passes through: a page with
   a null row keeps the null row in the concat output, and
   its validity bit is preserved.
6. Union order is stable: running the same input twice
   produces byte-identical `(dict_offsets, dict_data)`.
7. Empty-dict page edge case: a page with zero dict entries
   and zero rows concatenates without error.
8. Single-page input → fast-path short-circuit: output is
   identical to input (no union work).
9. Mixed physical-type input (one Utf8, one DictIndex32) →
   error. `concat_dict_pages` requires all pages to be
   `DictIndex32`; the reader's outer `concat_buffers` still
   dispatches on type.
10. **Honest coverage note.** No test exercises the union
    path through the full `SegmentReader::read_column` API,
    because no v1 writer produces differing-dict pages. All
    §3.5 items 3–7 drive `ColumnBuffer::concat_dict_pages`
    directly. This is the designed coverage surface; a
    future per-page-sub-dict writer ADR would add end-to-end
    tests through the writer → reader → concat path.

## 4. Alternatives considered

**A. Do nothing; keep Utf8 fallback.** The fallback is
unreachable today anyway. Cons: when the future per-page-
sub-dict writer lands, differing-dict concat would silently
promote to `Utf8`, breaking any caller that depends on the
`DictIndex32` output shape. ADR 0084 lands the union now so
the writer ADR has a stable reader behavior to depend on.

**B. Require all writers to produce identical dicts
(formalize the ADR 0082 §3.1 invariant).** Cons: forecloses
per-page sub-dict optimization permanently; forces every
writer to duplicate the full column dict into every page
(the problem ADR 0083 solved for operators willing to opt
into v1.4). Rejected.

**C. New reader method `read_column_as_utf8` that always
materializes to Utf8.** Orthogonal — useful for callers that
don't want dict-encoded output. Doesn't address the "what
does concat return" question. Deferred to a separate ADR.

**D. Keep the Utf8 fallback and add dict-union as an
opt-in method.** Two code paths, two sets of tests, two
output shapes depending on an operator-held flag.
Unnecessary complexity for an unreachable-today case.
Rejected.

**E. Streaming union (pages consumed as an iterator).**
Deferred to a future streaming-reads ADR. The current
reader materializes all pages before concat.

## 5. Open design questions (owner review)

1. **Public API surface for `concat_dict_pages`.** Draft:
   public method on `ColumnBuffer` so tests drive the real
   union code path. Alternative: `pub(crate)` + an in-tree
   test. Confirm.
2. **Union order determinism.** Draft: order of first
   appearance across pages → entries within page in dict-
   offset order. Confirm this is the right stability
   contract.
3. **Empty-dict / zero-row pages.** Draft: allow them; their
   contribution to the union is empty. Confirm.
4. **Null sentinel representation.** Draft: `u32::MAX` passes
   through unchanged. Alternative: normalize nulls at the
   union step. Rejected — `u32::MAX` is already the
   contract.
5. **HashMap<Vec<u8>, u32> for entry→new_idx lookup.**
   Draft: fine. Alternative: use the dict-builder's pattern
   (ahash::AHashMap). Confirm whether the dependency is
   warranted here.
6. **Mixed-type concat error vs promotion.** Draft: outer
   `concat_buffers` dispatches on physical type; mixed types
   are already a corruption error. Confirm no change.
7. **Error vs panic on impossible branches.** Draft:
   `unreachable!()` after the matches! guard. Alternative:
   explicit error. Confirm this is idiomatic.

## 6. Impact on invariants

- **INV-001/002/003/004:** N/A — `.brcs` is a durable
  analytical format.
- **INV-005 recoverable lifecycle:** reader-only change. No
  format version bump. No feature flag. Existing v1.0–v1.4
  segments read back with byte-identical results today
  (the fast path applies).

## 7. Dependencies this ADR would add

None (uses existing HashMap + Bitmap types).

## 8. What this ADR explicitly does NOT do

- Change any on-disk format.
- Introduce any writer behavior change.
- Introduce streaming reads.
- Add CLI exposure.
- Change ADR 0083 v1.4 column-dict decode (that path still
  uses a single shared dict, no union needed).

## 9. Acceptance evidence required before marking Accepted

1. ADRs 0060–0083 accepted — **satisfied**.
2. Owner sign-off on §5 open design questions, especially
   Q1 (public API) and Q2 (union order determinism).
3. Implementation satisfying every item in §3.5.
4. All pre-ADR-0084 tests (1076 workspace tests) pass
   unchanged.
5. New §3.5 tests added, all passing.
6. No regressions in any bradian-storage, bradian-relational, or
   workspace test.

Only then can this ADR move from **PROPOSED** to **ACCEPTED**.

## 10. Honest forward-compat note

This ADR ships code that is unreachable through the current
writer path. Its **sole purpose** is to give a future per-
page-sub-dict writer ADR a stable, well-defined reader
contract to depend on. The claim is NOT "operators see
smaller files today" (they don't; nothing changes). The claim
is "when a future writer emits differing-dict pages, the
reader will union them into one `DictIndex32` instead of
materializing to `Utf8`, preserving the dict-encoded output
shape."

If no per-page-sub-dict writer ADR ever lands, this code
remains dead but harmless. The §3.5 unit tests drive it
directly, so it is never untested; it is only unused in
end-to-end flows.
