# ADR 0191 — JSON array index segments in path (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-22.** Follow-up to ADR 0186 /
0188 / 0189 / 0190. Closes the "array index" open item.

## 1. Scope

- New `JsonSegment { Key(String), Index(usize) }` enum
  replaces the raw `Vec<String>` path field on
  `Projection::JsonPath`.
- `parse_projection_tail` accepts a non-negative integer
  literal as a segment after `->` or `->>`. Rejects
  negative, floating-point, and any other token with
  `SqlError::Syntax`.
- `apply_projection` navigates each segment via
  `JsonSegment::apply`, which wraps
  `serde_json::Value::get(&str)` for keys and
  `serde_json::Value::get(usize)` for indices.
- Label rendering: key as `'key'`, index as its decimal
  form, both preceded by `->` or `->>`.
- Out-of-bounds or wrong-shape navigation
  (index on object, key on array) yields `Value::Null`
  — same semantics as "missing key" under ADR 0186.

All four SQL engines pick up array indices automatically
since they use the shared `parse_projection` /
`apply_projection` helpers.

## 2. Out of scope

- **Negative indices** (`col -> -1` to count from the
  end). Rejected at parse.
- **Slices** (`col -> 0:3`). Not supported.
- **Mid-chain `->>`**. Same rule as ADR 0189.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/json_path_array_index.rs`
(9 tests, all green):

1. `arrow_arrow_array_index_returns_text` —
   `body -> 'tags' ->> 1` returns `"green"`.
2. `arrow_array_index_returns_json` — `body -> 'tags'
   -> 0` returns the JSON sub-value.
3. `chained_index_and_key_in_one_path` —
   `body -> 'tags' -> 1 ->> 'c'`.
4. `out_of_bounds_index_returns_null`.
5. `empty_array_index_returns_null`.
6. `where_filter_by_array_index_text`.
7. `negative_index_is_rejected_at_parse`.
8. `float_index_is_rejected_at_parse`.
9. `applying_index_to_object_returns_null`.

ADR 0186 test `path_rejects_non_text_key_literal`
updated to assert rejection of float and negative
indices (integers are now valid). ADR 0186 / 0188 /
0189 / 0190 suites continue to pass unchanged. Clippy
clean on `-D warnings`.

## 4. Roadmap impact

BRD-REL-004 JSON surface after this ADR:

| Capability | Status |
|---|---|
| `Value::Json` round-trip | ✓ ADR 0173 |
| SELECT projection `->` / `->>` single key | ✓ ADR 0186 + 0188 |
| SELECT chained paths | ✓ ADR 0189 |
| WHERE equality on path | ✓ ADR 0190 |
| **Array index in path** | **✓ ADR 0191** |
| JSON mutation on UPDATE | open — ADR 0192 next |
| `#>` / `#>>` container operators | open |
| Negative / slice indices | open |
