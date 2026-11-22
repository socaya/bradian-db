# ADR 0200 — JSON element removal on UPDATE SET (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-22.** First of the extension
items noted after ADR 0199.

## 1. Scope

- New `REMOVE` keyword that appears after a JSON path LHS
  in UPDATE SET, dropping the `= literal` RHS:
  - `SET col -> 'arr' -> N REMOVE` — deletes array index
    N, shifting the remaining elements left.
  - `SET col -> 'obj' -> 'k' REMOVE` — drops the key `k`
    from the object.
  - `SET col #> '{path}' REMOVE` — container-form works.
- New `SetAssignment::JsonRemove { column, path }` and
  `HistoryAssignment::JsonRemove { column, path }`
  variants.
- New `sql::apply_json_path_remove(column, cell, path)` +
  `descend_and_remove` helper that walks the path and
  removes the terminal element.
- Wired through SqlEngine, DomainSqlEngine,
  MvccSqlEngine, and HistorySqlEngine UPDATE paths.
  Removes run **after** column assignments and JSON path
  mutations in the same statement, so `SET body ->
  'new' = 'added', body -> 'name' REMOVE` ends with
  `name` gone and `new` present.

Order within a SET list: Column assignments → JSON path
mutations → JSON removes. Each step runs CHECK / FK /
UNIQUE against the *final* mutated row per matched
target.

## 2. Out of scope

- **REMOVE on a terminal `Slice` segment**. Rejected
  with `SqlError::Unsupported` (ambiguous — would
  require array splicing semantics).
- **REMOVE on a mid-chain segment**. Not supported; the
  REMOVE keyword always applies to the terminal
  segment.
- **Prepend / splice / concat**. Separate ADRs.
- **`DELETE` keyword reuse**. Chose `REMOVE` to avoid
  confusion with row-level DELETE.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/json_path_remove.rs`
(12 tests, all green):

1. `remove_object_key`.
2. `remove_missing_object_key_is_idempotent` — no-op.
3. `remove_array_element_shifts_left`.
4. `remove_last_element_by_negative_index`.
5. `remove_via_container_form` — `#> '{tags,0}'`.
6. `remove_array_index_out_of_bounds_is_rejected`.
7. `remove_slice_segment_is_rejected`.
8. `remove_with_json_path_where` — combined with the
   ADR 0199 WHERE grammar.
9. `remove_combines_with_other_assignments_in_one_set`
   — `SET … = …, … REMOVE`.
10. `remove_on_non_json_column_fails_cleanly`.
11. `domain_engine_remove` — DomainSqlEngine.
12. `mvcc_engine_remove` — MvccSqlEngine.

All prior JSON path suites (ADRs 0186–0199) continue to
pass. Clippy clean on `-D warnings`.

## 4. Roadmap impact

BRD-REL-004 JSON surface after this ADR:

| Capability | Status |
|---|---|
| …everything from ADRs 0173–0199 | ✓ |
| **JSON element REMOVE on UPDATE** | **✓ ADR 0200** |
| JSON array insert / prepend / concat | open |
| Mixed AND combinators in WHERE | open |
