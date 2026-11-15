# ADR 0087 — v1.4 reader single-clone column-dict fast path (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-15 and implemented.**

Blanket pre-approval from the owner to continue the draft+
implement cycle for ADRs 0086–0088. This ADR is the narrowest-
scope member of that batch.

## 1. Context

- ADR 0083 reader's `decode_page` clones the v1.4 column-dict
  `(dict_offsets, dict_data)` INTO each per-page
  `ColumnBuffer::DictIndex32`. For an N-page column that's N
  clones of the same bytes.
- ADR 0084 `concat_dict_pages` fast path then compares the N
  clones byte-wise, confirms they match, keeps page 0's
  clone, drops the remaining N-1. Wasted memory + wasted
  comparison.
- Full `Arc<DictBytes>` sharing in `ColumnBuffer` would touch
  46 pattern-match sites across the workspace with limited
  payoff — ADR 0082 §4(D) deferred that work.

## 2. Scope of this ADR

**In scope:**

- New reader-internal fast path in `read_column`: when the
  column has a v1.4 column-dict entry, clone the dict ONCE,
  decode each page as `indices + validity` only, and build a
  single `ColumnBuffer::DictIndex32` with the one-time
  clone. No `concat_dict_pages` call needed.
- Pre-existing non-v1.4 paths unchanged.
- Preserves the ADR 0083 §3.4 CRC + index-bounds checks per
  page.
- Measurable savings: 1 dict clone per column instead of N.
  For a 512-page column with ~8 KB dict, that's ~4 MB less
  allocator traffic per read_column call.

**Explicitly out of scope** (future ADRs):

- **Arc<DictBytes> in `ColumnBuffer` public API.** Still
  deferred. The ADR 0087 fast path reduces clones at the
  reader boundary; the output buffer still owns its one
  copy.
- **Lazy dict load.** The dict is still read eagerly during
  `open_with_bace`.
- **Shared-dict scans across multiple columns.**
- **Streaming reads.** ADR 0088 covers that.

## 3. Design

New method `SegmentReader::read_v14_dict_column`:
1. Look up `column_dicts.get(name)`; if absent, fall back
   to the default `read_column` path.
2. Clone the dict once (`shared_offsets`, `shared_data`).
3. Walk every page: decode payload, validate CRC, call
   `dict::decode_indices_only`, cross-check `n_dict_declared`
   against the actual dict size, validate indices in-range,
   concat into one `indices` vec + one `validity` bitmap.
4. Return one `ColumnBuffer::DictIndex32` with the
   `shared_*` bytes installed.

`read_column` detects v1.4-mode columns (Dictionary codec +
present column_dict entry) and delegates to the new helper.

## 3.5 Compliance

1. v1.4 segment with a single-page dict column: output
   matches pre-ADR-0087 byte-identically.
2. v1.4 segment with a 4-page dict column: same.
3. v1.3 (non-column-dict) dict column: unchanged path.
4. All existing ADR 0083 `column_dict.rs` tests pass.
5. CRC mismatch on the column-dict region still rejected.
6. Clippy clean.

## 4. Alternatives considered

**A. Arc<DictBytes> in ColumnBuffer.** Deferred per §2.
46 pattern-match sites to update with uncertain payoff.

**B. Zero-copy by lifetime-borrowing dict bytes from the
reader.** Changes `ColumnBuffer` into a borrowing type.
Massive API ripple. Rejected.

**C. Do nothing.** The ADR 0084 fast path is already correct;
the concern is pure allocator traffic. Rejected — small
diff, clear win.

## 5. Open design questions

None substantive; the implementation is a mechanical
rearrangement of existing code paths.

## 6. Acceptance evidence

1. All existing 1096 workspace tests pass.
2. Clippy clean.
3. A new ADR-0087 micro test verifies that reading a 4-page
   v1.4 dict column produces identical output to the
   pre-ADR-0087 code path.
