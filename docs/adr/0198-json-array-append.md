# ADR 0198 — JSON array append via index-at-length write (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-22.** Closes the "array append"
open item flagged in ADRs 0191 / 0192 / 0196 / 0197.

## 1. Scope

- `descend_and_assign` grows a special terminal-segment
  branch: if the LHS's **last** segment is `Index(n)` and
  `n == arr.len()`, the leaf is **appended** via
  `arr.push(new_leaf)`.
- `n < arr.len()` still replaces in place (unchanged from
  ADR 0192).
- `n > arr.len()` still fails with `SqlError::Type` — no
  sparse arrays.
- `IndexFromEnd(_)` cannot append; it always addresses an
  existing element (so it fails if out of bounds).
- Mid-chain `Index(n)` with `n == len` still rejects
  (creating a hypothetical slot and descending into it
  would silently create an empty object container, which
  is inconsistent with the "arrays cannot be auto-
  extended" invariant for intermediates).
- Works in both the `->` chain and the `#>` /
  `#>>` container form (the segment list lowers to the
  same path type).
- `->>` at the terminal still writes a JSON-string leaf,
  so appending `42` through `->>` produces `"42"` in the
  array.

## 2. Out of scope

- **Prepend** (`SET col -> 'arr' -> 0 = lit` already
  replaces, not prepends).
- **Splice/insert mid-array**.
- **Array remove**.
- **Array concat** (`arr || arr`).

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/json_path_array_append.rs`
(9 tests, all green):

1. `append_via_index_at_length` — len=2 → push to
   index 2.
2. `append_to_empty_array` — index 0 on empty array
   pushes.
3. `multiple_appends_build_up_the_array`.
4. `append_with_arrow_arrow_text_writes_string_tag`
   — numeric literal under `->>` becomes `"42"`.
5. `append_via_container_form` — `#> '{tags,2}'`.
6. `index_one_past_length_still_rejects` — len=2, idx=3.
7. `negative_index_cannot_append`.
8. `midchain_index_at_len_is_still_rejected`.
9. `append_round_trips_via_negative_last_read` —
   `-> 2 = 'blue'` then `->> -1` reads `"blue"`.

ADR 0192 suite (12 tests — including
`set_arrow_array_index_out_of_bounds_is_rejected` which
uses idx=5 on len=1) continues to pass. All prior JSON
path suites (ADRs 0186 / 0188–0197) pass. Clippy clean on
`-D warnings`.

## 4. Roadmap impact

BRD-REL-004 JSON surface after this ADR:

| Capability | Status |
|---|---|
| `Value::Json` round-trip | ✓ 0173 |
| SELECT projection paths (single, chained, container, array, negative, slice) | ✓ 0186–0197 |
| WHERE equality + non-equality on paths | ✓ 0190 / 0195 |
| UPDATE SET JSON path mutation (all four engines) | ✓ 0192 / 0193 |
| `#>` / `#>>` container operators | ✓ 0194 |
| Negative array indices | ✓ 0196 |
| JSON array slice | ✓ 0197 |
| **JSON array append (terminal `Index(len)` write)** | **✓ ADR 0198** |
| JSON path in UPDATE/DELETE WHERE | open |
| Array prepend / splice / remove / concat | open |
