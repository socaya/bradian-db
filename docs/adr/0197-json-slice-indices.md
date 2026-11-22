# ADR 0197 — JSON array slice indices (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-22.** Closes the "slice
indices" open item from ADRs 0191 / 0192 / 0196.

## 1. Scope

- New `JsonSegment::Slice { start: usize, end: usize }`
  with Python-style end-exclusive semantics.
- Lexer: `:` admitted to the symbol set (ADR 0183's named-
  parameter path runs at the driver layer before SQL
  reaches this lexer, so this does not conflict).
- `parse_projection_tail`: after `->` / `->>`, a non-
  negative integer followed by `:` followed by another
  non-negative integer produces a `Slice` segment.
  Missing `end` (`2:`), missing `start` (`:3`), and
  `start > end` all rejected at parse.
- `parse_container_path` (`#>` / `#>>`): a brace segment
  `start:end` with both bounds non-negative lowers to
  the same `Slice` variant.
- `JsonSegment::apply_slice` returns a fresh
  `serde_json::Value::Array` of the clamped
  `start..end.min(len)` range; non-array value → `None`
  → `Value::Null`.
- `apply_projection` dispatches to `apply_slice` when it
  encounters a `Slice` segment, cloning the sub-array;
  chaining past a slice works (e.g. `-> 1:4 ->> 0`).
- **Mutation**: a `Slice` segment on an UPDATE SET LHS
  returns `SqlError::Unsupported`. Replacing a sub-
  array needs separate semantics and is deferred.

## 2. Out of scope

- **Open-ended bounds** (`N:`, `:M`). Both bounds
  required.
- **Negative bounds** in a slice (`-3:-1`). Deferred.
- **Step/stride** (`0:10:2`). Not supported.
- **Slice mutation**. Deferred.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/json_path_slice.rs`
(12 tests, all green):

1. `arrow_slice_returns_sub_array` — `tags -> 1:4` →
   `["green","blue","yellow"]`.
2. `arrow_slice_from_zero_matches_prefix`.
3. `arrow_slice_past_end_clamps`.
4. `arrow_slice_equal_bounds_returns_empty_array`.
5. `arrow_slice_on_empty_array_returns_empty_array`.
6. `arrow_slice_on_non_array_returns_null`.
7. `chained_slice_then_index_extracts_single_element`
   — `-> 1:4 ->> 0`.
8. `container_slice_segment` — `#> '{tags,1:3}'`.
9. `slice_start_greater_than_end_rejected_at_parse`.
10. `slice_missing_end_rejected`.
11. `slice_container_bad_form_rejected` — `'{tags,1:}'`.
12. `slice_on_update_lhs_is_rejected` —
    `SqlError::Unsupported`.

Full JSON path regression (ADRs 0186 / 0188 / 0189 /
0190 / 0191 / 0192 / 0193 / 0194 / 0195 / 0196) still
green; `sql_subset_corpus` (46 tests) still green.
Clippy clean on `-D warnings`.

## 4. Roadmap impact

BRD-REL-004 JSON surface after this ADR:

| Capability | Status |
|---|---|
| `Value::Json` round-trip | ✓ 0173 |
| SELECT projection paths | ✓ 0186 / 0188 / 0189 / 0191 / 0194 / 0196 |
| **SELECT slice segments** | **✓ ADR 0197** |
| WHERE equality + non-equality on paths | ✓ 0190 / 0195 |
| UPDATE SET JSON path mutation (all four engines) | ✓ 0192 / 0193 |
| `#>` / `#>>` container operators | ✓ 0194 |
| Negative array indices | ✓ 0196 |
| JSON array append / splice | open — ADR 0198 next |
| JSON path in UPDATE/DELETE WHERE | open |
