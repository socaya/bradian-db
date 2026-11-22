# ADR 0193 — JSON path mutation in HistorySqlEngine UPDATE (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-22.** Closes the deferred
HistorySqlEngine strand from ADR 0192 §2.

## 1. Scope

- New `sql_expression::HistoryAssignment` enum with two
  variants:
  - `Expression { column, expr }` — existing numeric
    expression RHS.
  - `JsonPath { column, path, as_text, value }` —
    ADR 0192's path assignment.
- New `sql_expression::mixed_assignments` SET parser
  that decides per-element: if the token after the
  column name is `Arrow` or `ArrowText`, dispatch to
  `parse_projection_tail` + literal; otherwise parse the
  existing arithmetic expression.
- New `Parser::next_is_json_arrow()` predicate exposes
  the Arrow token test without leaking the private
  `Token` enum to `sql_expression`.
- `HistorySqlEngine::execute_inner` UPDATE wires
  `mixed_assignments`:
  - Expression replacements run first (evaluated against
    the original row — same ADR 0043 semantics).
  - JSON path mutations run next against the already-
    expression-updated row, via the same shared
    `apply_json_path_mutation`.
- Downstream CHECK / FK / UNIQUE constraints run on the
  final mutated row, unchanged.

## 2. Out of scope

- **Chained JSON mutation across multiple columns in
  one SET**. Only one leaf per path. Per-column multiple
  JSON mutations are possible (the vector is iterated in
  order).
- **Path mutation on `->>` where the leaf would need
  arithmetic evaluation** — RHS remains a Literal; the
  expression grammar isn't reused for path leaves.
- **`DEFAULT` on a path leaf** — rejected at parse (same
  as ADR 0192).
- **Negative or slice indices** (open since ADR 0191).
- **Array append / splice** (open).

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/json_path_mutation_history.rs`
(4 tests, all green):

1. `history_set_arrow_key_replaces_scalar_subvalue` —
   `UPDATE … SET body -> 'name' = 'bob'`.
2. `history_mixes_expression_and_path_mutation_in_one_set`
   — `SET amount = amount + 5, body -> 'k' = 99` in one
   UPDATE: arithmetic expression on `amount` and path
   mutation on `body` both apply.
3. `history_chained_path_auto_creates_intermediate` —
   missing `body -> 'meta'` auto-creates an empty
   object before setting `meta -> 'origin' = 'test'`.
4. `history_arrow_arrow_writes_json_string` — `->>` on
   a mutation leaf writes a JSON-string value.

ADR 0192 suites (12 tests) continue to pass. The ADR
0043 historical UPDATE expression suite continues to
pass — `mixed_assignments` is a strict superset of the
old `assignments`. Clippy clean on `-D warnings`. Old
numeric-only `assignments()` entry point removed (no
callers after this slice).

## 4. Roadmap impact

BRD-REL-004 JSON surface after this ADR:

| Capability | Status |
|---|---|
| `Value::Json` round-trip | ✓ ADR 0173 |
| SELECT projection paths (single, chained, array) | ✓ ADRs 0186 / 0188 / 0189 / 0191 |
| WHERE-side equality on paths | ✓ ADR 0190 |
| UPDATE SET JSON path mutation | ✓ ADR 0192 (SqlEngine, Domain, Mvcc) |
| **HistorySqlEngine UPDATE JSON path mutation** | **✓ ADR 0193** |
| `#>` / `#>>` container-path operators | open |
| JSON path in UPDATE/DELETE WHERE | open |
| Negative / slice indices | open |
| Non-equality on JSON path | open |
| JSON array append / splice | open |
