# BACE dict-union concat verification (ADR 0084)

15 November 2026. Baseline: `e040449`. Reader-only change: the
ADR 0082 §3.3 Utf8 fallback for differing-dict `DictIndex32`
pages is retired. The reader now unions per-page dicts into one
by first-appearance order and remaps indices, preserving the
`DictIndex32` output shape even for differing-dict input.

## Executed result

**1086 Rust tests passed, 0 failed, 0 ignored** (1076 → 1086).
Clippy clean on `cargo clippy --workspace --all-targets
--locked -- -D warnings`.

| Executed check | Result |
|---|---|
| `cargo check -p dap-storage --locked` | Passed |
| `cargo test -p dap-storage --test dict_union --locked` | 10 passed |
| `cargo test --workspace --locked` | 1086 passed, 0 failed |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed |

## What landed

### R.1 — `ColumnBuffer::concat_dict_pages`

New public method on `ColumnBuffer` in `column.rs`:
- Rejects empty input + mixed physical types with a
  `Corruption` error.
- Single-page input → returns the page verbatim.
- Fast path: identical dict bytes across every page →
  concat indices + validity, clone dict once.
- Slow path: build union dict by first-appearance across
  pages (entries within a page in dict-offset order), remap
  each page's indices to the union numbering, concat.
- Null sentinel `u32::MAX` passes through unchanged.
- Union lookup via `HashMap<Vec<u8>, u32>`.

### R.2 — reader delegation

`reader.rs::concat_buffers` for `PhysicalType::DictIndex32`
now delegates to `ColumnBuffer::concat_dict_pages`. The
ADR 0082 Utf8 fallback branch is removed. ADR 0082's
identical-dict fast path lives inside `concat_dict_pages`
and behaves identically — no byte or shape change for any
writer-produced v1.0–v1.4 segment.

### 10 §3.5 integration tests in `dict_union.rs`

| # | Test | §3.5 item |
|---|---|---|
| 1 | `item_1_existing_adr_0082_fast_path_untouched` | fast path |
| 2 | `item_2_identical_dicts_fast_path_preserves_dict_bytes` | 2 |
| 3 | `item_3_non_overlapping_dicts_produce_unioned_output` | 3 |
| 4 | `item_4_partial_overlap_dedups_shared_entries` | 4 |
| 5 | `item_5_null_sentinel_passthrough` | 5 |
| 6 | `item_6_union_order_is_stable_across_runs` | 6 |
| 7 | `item_7_empty_dict_page_tolerated` | 7 |
| 8 | `item_8_single_page_input_is_returned_verbatim` | 8 |
| 9 | `item_9_mixed_physical_type_input_is_rejected` | 9 |
| 10 | `item_10_three_page_union_round_trips` | 10 |

## Compliance against ADR 0084 §9

1. ADRs 0060–0083 accepted — ✓
2. Owner sign-off on §5 open design questions — accepted as
   drafted:
   - Q1: public API on `ColumnBuffer` so tests drive the real
     union code.
   - Q2: order = first appearance across pages → dict-offset
     order within page; stable across runs (test item 6).
   - Q3: empty-dict + zero-row pages tolerated (test item 7).
   - Q4: `u32::MAX` null sentinel passes through unchanged.
   - Q5: `HashMap<Vec<u8>, u32>` is fine; no ahash
     dependency added.
   - Q6: mixed-type input rejected via `Corruption`
     (test item 9).
   - Q7: `unreachable!()` after `matches!` guard is idiomatic.
3. Every §3.5 item 2–10 (plus the "item 1 untouched" marker)
   covered by at least one test.
4. All pre-ADR-0084 tests (1076) pass unchanged.
5. 10 new tests added, all passing.
6. No regressions in any workspace crate.

## §10 honest forward-compat note (unchanged)

This ADR ships code that is unreachable through the current
writer path. ADR 0082 §3.1 writer invariant (every page's
dict is byte-identical) still holds; the identical-dict fast
path still fires for every writer-produced segment. The
slow dict-union path exists only for a future per-page-sub-
dict writer ADR. Until then, the code is tested via unit
tests that construct differing-dict inputs by hand.

## BACE workstream progress (as of this ADR)

| ADR | Component | Status |
|---|---|---|
| 0060–0066 | Prerequisites + RowStore + ZSTD | ACCEPTED |
| 0067–0068 | Backup carrier adapters | ACCEPTED |
| 0069–0072 | All four SQL engines BACE open | ACCEPTED |
| 0073 | `bradian-db` CLI BACE flags | ACCEPTED |
| 0074 | G23 DDL demonstration | ACCEPTED |
| 0075 | BACA envelope primitive | ACCEPTED |
| 0076 | `.derc` carrier BACE (v1.1) | ACCEPTED |
| 0077 | BRWL carrier BACE (benchmarked) | ACCEPTED |
| 0078 | BACE dictionary primitive | ACCEPTED |
| 0079 | `.derc` dictionary carrier (v1.2) | ACCEPTED |
| 0080 | `.derc` BACA carrier (v1.3) | ACCEPTED |
| 0081 | `.derc` multi-page writer | ACCEPTED |
| 0082 | Multi-page DictIndex32 columns | ACCEPTED |
| 0083 | Column-level shared dict (v1.4) | ACCEPTED |
| **0084** | **Dict-union concat (forward-compat)** | **ACCEPTED** |

## Baseline + commit

Baseline: `e040449` (ADR 0083 ACCEPTED).
Implementation commit: assigned by this commit.

See [ADR 0084](../adr/0084-derc-dict-union-concat.md).
