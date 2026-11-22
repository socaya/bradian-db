# ADR 0192 — JSON path mutation on UPDATE SET (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-22.** Closes the "JSON mutation
on UPDATE" open item flagged in ADR 0186 §2 and later ADRs.

## 1. Scope

- **Grammar**: UPDATE SET now accepts bare-column and JSON
  path assignments in the same list:
  - `SET col = <literal>` — existing column replacement.
  - `SET col -> 'k' [...] = <literal>` — JSON path
    assignment that preserves the surrounding sub-tree.
  - `SET col ->> 'k' [...] = <literal>` — same, but the
    leaf is written as a canonical text string.
- Path chains (ADR 0189) and array indices (ADR 0191) are
  reused — the LHS uses the same `parse_projection_tail`
  helper as SELECT.
- **Evaluation** (`apply_json_path_mutation`):
  - Source cell `NULL` promotes to `{}` when the first
    segment is a key (assign-creates-container
    semantics).
  - Missing key intermediate steps auto-create empty
    objects.
  - Missing index intermediate / out-of-bounds index →
    `SqlError::Type` (arrays cannot be auto-extended in
    this slice).
  - Replacing a non-object intermediate with an object
    is the SQL-standard "assign-creates-container"
    behavior.
  - Leaf write: `->` renders the literal via
    `serde_json`; `->>` writes the literal as a
    JSON-string value.
- **Order of operations** on a single UPDATE: all bare-
  column replacements first, then all JSON path
  mutations. Lets `SET body = '{}', body -> 'k' = 1`
  mean "reset to `{}` and then set `k=1`".
- Wired into `SqlEngine`, `DomainSqlEngine`,
  `MvccSqlEngine`.

## 2. Out of scope

- **`HistorySqlEngine` UPDATE**. History uses
  `sql_expression::assignments` (numeric expression RHS);
  wiring JSON path mutation there requires extending the
  expression grammar and is deferred.
- **PK-column JSON mutation**. UPDATE already rejects
  any assignment to a PK column; the same rule applies
  to a path LHS whose column is in the PK.
- **DELETE of a sub-value** (`SET col -> 'k' = DEFAULT`
  or `UNSET`). Only replacement.
- **Array append / insert / remove**. Arrays can be
  replaced in-place; the length cannot change.
- **DEFAULT literals on the RHS** — rejected at parse.
- **Non-equality operators** or expression RHS in path
  assignment.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/json_path_mutation.rs`
(12 tests, all green):

1. `set_arrow_key_replaces_scalar_subvalue`.
2. `set_arrow_arrow_key_treats_literal_as_text` —
   `->>` writes a JSON string.
3. `set_arrow_with_numeric_literal_writes_json_number`.
4. `set_arrow_missing_key_creates_it`.
5. `set_arrow_chained_key_auto_creates_intermediate_object`.
6. `set_arrow_array_index_in_bounds_replaces_element`.
7. `set_arrow_array_index_out_of_bounds_is_rejected`.
8. `mix_bare_column_and_json_path_assignments` — one
   UPDATE sets both a plain column and a JSON path.
9. `reset_column_then_mutate_path_runs_in_order` — `SET
   body='{}', body -> 'k' = 1` yields `{"k":1}`.
10. `applying_json_path_to_non_json_column_fails_cleanly`.
11. `domain_engine_json_path_mutation`.
12. `mvcc_engine_json_path_mutation`.

Existing JSON path suites (0186 / 0188 / 0189 / 0190 /
0191) continue to pass; `sql_subset_corpus` is unchanged.
Clippy clean on `-D warnings`.

## 4. Roadmap impact

BRD-REL-004 JSON surface after this ADR:

| Capability | Status |
|---|---|
| `Value::Json` round-trip | ✓ ADR 0173 |
| SELECT projection paths (single + chained + array) | ✓ ADRs 0186 / 0188 / 0189 / 0191 |
| WHERE-side equality on paths | ✓ ADR 0190 |
| **UPDATE SET JSON path mutation** | **✓ ADR 0192** |
| HistorySqlEngine UPDATE JSON path mutation | open |
| `#>` / `#>>` container-path operators | open |
| JSON path in UPDATE/DELETE WHERE | open |
| Negative / slice indices | open |
| Non-equality on JSON path | open |
| JSON array append / splice | open |
