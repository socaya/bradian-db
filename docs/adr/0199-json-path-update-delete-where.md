# ADR 0199 — JSON path predicates in UPDATE/DELETE WHERE (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-22.** Closes the last of the
BRD-REL-004 JSON path open items. UPDATE and DELETE now
accept the same WHERE grammar that SELECT does.

## 1. Scope

All four SQL engines' UPDATE and DELETE paths switch from
`key_predicate(&mut p, &table)?` (PK-only) to
`parse_select_where(&mut p, &table)?` (PK equality OR
JSON path predicate).

Dispatch:
- `SelectWhere::KeyPredicate(pk)` keeps the single-row
  fast lookup via `tx.get(table.id, pk)` /
  `store.get(pk)`.
- `SelectWhere::JsonPathPredicate { projection, op,
  literal }` scans the table once, runs
  `apply_where_filter`, and processes every matching
  row.

Per-row work preserved exactly as before:
- UPDATE: apply column assignments → apply JSON path
  mutations → `check_row` → `foreign_key_check` →
  UNIQUE enforcement (PK-filtered against the matched
  row's PK) → `tx.replace` / `store.replace`.
- DELETE: `fk_apply_parent_delete_tx` (or
  `foreign_key_restrict` on `SqlEngine`) per matched
  row → `tx.delete` / `store.delete`.

The `affected` count is now the number of rows matched,
which is 0 or 1 when the WHERE is a PK predicate (same
as before) and 0..N when the WHERE is a JSON path
predicate.

PK-column assignment on UPDATE still rejects regardless
of WHERE form (`primary key UPDATE not supported`).

## 2. Out of scope

- **Multi-row UPDATE/DELETE with non-PK, non-JSON
  predicates** (e.g. `WHERE label = 'x'`). Still
  rejected by `parse_select_where`'s grammar because
  the WHERE dispatcher only enters the JSON-path branch
  on an Arrow / ArrowText / Hash / HashText token after
  the first column — bare column equality still requires
  the full PK.
- **Mixed AND combinators** (`WHERE pk = N AND body ->
  'k' = 'v'`). Deferred.
- **Expression RHS of UPDATE SET under JSON path
  WHERE on non-history engines.** `SqlEngine`,
  `DomainSqlEngine`, `MvccSqlEngine` still require a
  literal RHS for column assignments; only
  `HistorySqlEngine` has the ADR 0193 expression form.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/json_path_where_mutation.rs`
(12 tests, all green):

SqlEngine (9):
1. `update_with_json_path_where_affects_matching_rows`
   — `WHERE body ->> 'tier' > '2'` matches 2 rows.
2. `update_with_json_path_equality_matches_single_row`.
3. `update_with_json_path_where_no_match_returns_zero`.
4. `update_with_pk_equality_still_works_after_refactor`
   — ADR 0177 regression.
5. `update_with_json_path_rejects_pk_column_assignment`.
6. `delete_with_json_path_where_removes_matching_rows`.
7. `delete_with_json_path_equality`.
8. `delete_pk_equality_still_works` — ADR 0184
   regression.
9. `update_json_path_set_mutation_also_supported` —
   ADR 0192's SET JSON path works on top of ADR 0199's
   WHERE JSON path (`SET body -> 'tier' = 99 WHERE body
   ->> 'tier' = '2'`).

Cross-engine (3):
10. `domain_update_with_json_path_where`.
11. `history_update_with_json_path_where` — expression
    RHS + JSON path WHERE in one statement (`SET
    amount = amount + 5 WHERE body ->> 'n' = 'big'`).
12. `mvcc_delete_with_json_path_where`.

Full UPDATE/DELETE regression (unique_update_sql,
foreign_key_sql, foreign_key_session_engines,
foreign_key_actions_sql, check_constraint_sql,
json_path_mutation, json_path_mutation_history,
sql_subset_corpus) continues to pass. Clippy clean on
`-D warnings`.

## 4. Roadmap impact

BRD-REL-004 JSON surface after this ADR — all open
code-writable items are now closed:

| Capability | Status |
|---|---|
| `Value::Json` round-trip | ✓ 0173 |
| SELECT projection paths (single, chained, container, array, negative, slice) | ✓ 0186–0197 |
| WHERE equality + non-equality on paths (SELECT) | ✓ 0190 / 0195 |
| UPDATE SET JSON path mutation (all four engines) | ✓ 0192 / 0193 |
| `#>` / `#>>` container operators | ✓ 0194 |
| Negative array indices | ✓ 0196 |
| JSON array slice | ✓ 0197 |
| JSON array append | ✓ 0198 |
| **JSON path in UPDATE/DELETE WHERE** | **✓ ADR 0199** |

Remaining items in the gap register (array prepend /
splice / remove / concat) are out-of-scope extensions,
not follow-ups to listed open items.
