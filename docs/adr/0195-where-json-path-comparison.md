# ADR 0195 — Non-equality WHERE on JSON path (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-22.** Follow-up to ADR 0190 /
0194. Closes the "non-equality on JSON path" open item.

## 1. Scope

- `SelectWhere::JsonPathEquality` renamed to
  `SelectWhere::JsonPathPredicate` and extended with
  `op: crate::model::CheckOp`.
- `parse_select_where` reuses `Parser::word_or_symbol_pair_for_check`
  (introduced in ADR 0182 for CHECK constraints) to accept
  any of `=`, `<>`, `<`, `<=`, `>`, `>=` on a JSON-path LHS.
  Equality and container-form (ADR 0194) paths both route
  through the same op parser.
- `apply_where_filter` now takes the `CheckOp` and dispatches
  the right ordering check using the shared
  `compare_values_for_check` from ADR 0182.
- NULL on either side continues to never match (SQL
  three-valued logic — a comparison with NULL is UNKNOWN,
  treated as not-true), consistent with ADR 0190 equality.

## 2. Out of scope

- **Range predicates on bare non-JSON columns in WHERE**.
  ADR 0112 range-rejection still fires there because the
  normal PK-predicate parser only accepts `=`. The new op
  parsing is **inside** the JSON-path branch only — the
  dispatch already routed via a one-token lookahead
  (`Arrow` / `ArrowText` / `Hash` / `HashText`).
- **AND / OR combinators between JSON path predicates**.
  Single predicate, single column still.
- **BETWEEN / IN / LIKE on JSON path**. Deferred.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/json_path_where_cmp.rs`
(10 tests, all green):

1. `where_json_path_less_than_text`.
2. `where_json_path_greater_than_text`.
3. `where_json_path_less_or_equal`.
4. `where_json_path_greater_or_equal`.
5. `where_json_path_not_equal` — `<>`.
6. `where_json_path_numeric_text_compare_is_lexicographic`
   — `->>` is text; `'7' > '10'` lexicographically.
7. `where_json_path_container_form_with_comparison`
   — `body #>> '{name}' < 'carol'`.
8. `where_json_path_missing_key_never_matches_non_equality`.
9. `where_json_path_equality_still_works_after_refactor` —
   ADR 0190 regression.
10. `where_json_path_bad_op_fails_cleanly` — `~` rejected.

ADR 0190 WHERE suite (9 tests), ADR 0194 container suite
(12), ADR 0186/0188/0189/0191/0192/0193 suites all
continue to pass. Clippy clean on `-D warnings`. ADR 0112
range-rejection on non-JSON columns still passes
(`rejects_join_aggregate_and_or_and_range_and_alias`).

## 4. Roadmap impact

BRD-REL-004 JSON surface after this ADR:

| Capability | Status |
|---|---|
| `Value::Json` round-trip | ✓ ADR 0173 |
| SELECT projection paths | ✓ 0186 / 0188 / 0189 / 0191 |
| WHERE equality on paths | ✓ 0190 |
| **WHERE non-equality on paths** | **✓ ADR 0195** |
| UPDATE SET JSON path mutation (all four engines) | ✓ 0192 / 0193 |
| `#>` / `#>>` container operators | ✓ 0194 |
| Negative / slice indices | open |
| JSON array append / splice | open |
| JSON path in UPDATE/DELETE WHERE | open |
