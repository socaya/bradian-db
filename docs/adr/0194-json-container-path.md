# ADR 0194 — `#>` / `#>>` container-path operators (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-22.** Closes the open item from
ADRs 0186 / 0188 / 0189 / 0190 / 0191 / 0192 / 0193.

## 1. Scope

- **Lexer**: new tokens `Token::Hash` (`#>`) and
  `Token::HashText` (`#>>`). Emitted whenever `#` is
  followed by `>`.
- **Grammar**: a JSON path LHS may now start with either
  the existing `->` / `->>` chain (ADR 0186 + 0189) or a
  single `#>` / `#>>` container operator whose RHS is a
  text literal of the form `'{seg,seg,...}'`.
  - Segments are either bareword object keys or
    non-negative integer indices (ADR 0191).
  - Whitespace around commas and braces is tolerated.
  - Empty bodies (`'{}'`) and empty segments
    (`'{a,,b}'`) are rejected at parse.
- **Semantics**: identical runtime behavior to the
  equivalent arrow chain — the parser lowers both forms
  into the same `Projection::JsonPath.path` vector.
  `#>` ⇔ terminal `->`, `#>>` ⇔ terminal `->>`.
- **Label**: a new `container_syntax: bool` flag on
  `Projection::JsonPath` makes `Projection::label()`
  render the user's input faithfully
  (`body#>'{a,b,c}'` vs `body->'a'->'b'->'c'`).
- **Coverage**: all four SQL engines inherit the
  container form automatically because
  `parse_projection_tail` is the single parser used by
  SELECT projection, WHERE predicate
  (ADR 0190), UPDATE SET (ADR 0192) and HistorySqlEngine
  UPDATE SET (ADR 0193). `parse_select_where` and
  `path_assignments` were extended to recognize `#>` /
  `#>>` in the first-column lookahead.
- `Parser::next_is_json_arrow()` now also flags Hash /
  HashText so `sql_expression::mixed_assignments`
  (ADR 0193) accepts container-form mutation out of the
  box.

## 2. Out of scope

- **Chained container operators** (`col #> '{a}' #>
  '{b}'`). Not supported; one operator owns the full
  path for that column.
- **`#>` nested inside an arrow chain**. `col -> 'a'
  #> '{b,c}'` is rejected because `parse_projection_tail`
  treats the container operator as the leading form
  only.
- **Negative / slice indices** inside the brace body.
  Rejected at parse like elsewhere.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/json_path_container.rs`
(12 tests, all green):

1. `container_hash_returns_json_subvalue` — `body #>
   '{a,b}'` returns `Value::Json`.
2. `container_hash_hash_returns_text`.
3. `container_hash_supports_array_index_segments` —
   `body #>> '{tags,1}'` works.
4. `container_hash_missing_key_returns_null`.
5. `container_hash_is_whitespace_tolerant` — spaces
   around separators and braces.
6. `container_hash_rejects_missing_braces` —
   `#> 'a,b'` → `SqlError::Syntax`.
7. `container_hash_rejects_empty_body` — `#> '{}'`.
8. `container_hash_rejects_empty_segment` —
   `#> '{a,,b}'`.
9. `container_hash_in_where_filter` — `WHERE body #>>
   '{a,b,c}' = 'deep'`.
10. `container_hash_in_update_mutation` — `UPDATE …
    SET body #> '{a,b,c}' = 'changed'`.
11. `domain_container_hash` — DomainSqlEngine.
12. `mvcc_container_hash` — MvccSqlEngine.

All prior JSON path suites (ADRs 0186 / 0188 / 0189 /
0190 / 0191 / 0192 / 0193) continue to pass. Clippy
clean on `-D warnings`.

## 4. Roadmap impact

BRD-REL-004 JSON surface after this ADR:

| Capability | Status |
|---|---|
| `Value::Json` round-trip | ✓ ADR 0173 |
| SELECT projection paths | ✓ ADRs 0186 / 0188 / 0189 / 0191 |
| WHERE-side equality on paths | ✓ ADR 0190 |
| UPDATE SET JSON path mutation (all four engines) | ✓ ADRs 0192 / 0193 |
| **`#>` / `#>>` container-path operators** | **✓ ADR 0194** |
| Non-equality on JSON path | open |
| Negative / slice indices | open |
| JSON array append / splice | open |
