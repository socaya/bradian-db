# ADR 0196 — Negative JSON array indices (from-end) (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-22.** Reverses the "negative
indices rejected" boundary flagged in ADR 0191 §2.

## 1. Scope

- New `JsonSegment::IndexFromEnd(usize)` variant: `-1`
  addresses the last element, `-2` second-to-last, etc.
- `parse_projection_tail` accepts both the non-expression
  `Token::Number("-1")` form and the expression-mode
  `Token::Symbol('-') + Token::Number("1")` form, so
  HistorySqlEngine's expression-lexed sessions pick up
  negative indices too.
- `parse_container_path` accepts `-N` segments inside
  `#>` / `#>>` braces.
- `JsonSegment::apply` for `IndexFromEnd(n)` returns
  `None` when `n == 0`, `n > arr.len()`, or `value` is
  not an array.
- `JsonSegment::resolve_for_array(len)` resolves the
  segment to a concrete 0-based index for mutation; `None`
  on out-of-bounds.
- Mutation (`descend_and_assign`) collapses `Index` and
  `IndexFromEnd` into one array-write branch via
  `resolve_for_array`; both fail with `SqlError::Type` on
  out-of-bounds.
- `-0` rejected at parse (ambiguous with positive zero).

## 2. Out of scope

- **Array slices** (`col -> 0:3`). Separate ADR.
- **Negative container-path keys** that look numeric but
  are actually object keys. Same ambiguity as before —
  the parser cannot know; `-1` inside braces is always
  an `IndexFromEnd` segment.
- **Append / splice on arrays**.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/json_path_negative_index.rs`
(9 tests, all green):

1. `arrow_negative_one_returns_last_element`.
2. `arrow_negative_two_returns_second_to_last`.
3. `arrow_negative_out_of_bounds_returns_null`.
4. `arrow_negative_on_empty_array_returns_null`.
5. `container_path_accepts_negative_index` — `#>>
   '{tags,-1}'`.
6. `where_filter_by_negative_index` — `WHERE body ->
   'tags' ->> -1 = 'blue'`.
7. `update_set_negative_index_replaces_last_element`
   — `SET body -> 'tags' -> -1 = 'yellow'`.
8. `update_set_negative_index_out_of_bounds_is_rejected`.
9. `negative_zero_is_rejected_at_parse`.

Two existing stale assertions updated:
- `json_path_sql.rs::path_rejects_non_text_non_integer_key_literal`
  → `path_rejects_non_integer_key_literal`: the float
  case still rejects; the negative-index case moved to
  ADR 0196 coverage.
- `json_path_array_index.rs::negative_index_is_rejected_at_parse`
  → `negative_index_resolves_from_end_after_adr_0196`:
  negative index now returns the last element.

All prior JSON path suites continue to pass. Clippy clean
on `-D warnings`.

## 4. Roadmap impact

BRD-REL-004 JSON surface after this ADR:

| Capability | Status |
|---|---|
| `Value::Json` round-trip | ✓ 0173 |
| SELECT projection paths | ✓ 0186 / 0188 / 0189 / 0191 |
| WHERE equality + non-equality on paths | ✓ 0190 / 0195 |
| UPDATE SET JSON path mutation (all four engines) | ✓ 0192 / 0193 |
| `#>` / `#>>` container operators | ✓ 0194 |
| **Negative array indices** | **✓ ADR 0196** |
| Slice indices (`0:3`) | open |
| JSON array append / splice | open |
