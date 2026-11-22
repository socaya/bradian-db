# ADR 0186 — JSON path operators `->` and `->>` in SELECT (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-22.** Closes the ADR 0113 §2 /
ADR 0173 §2 "JSON path operators" follow-up. `SqlEngine`
SELECT only; mirror slice to the other three engines
remains open.

## 1. Scope

- **Lexer**: two new tokens `Token::Arrow` (`->`) and
  `Token::ArrowText` (`->>`). Emitted only when a `-` is
  followed by `>` at the top-level (non-expression) lex
  mode so the existing "`-` must prefix a numeric
  literal" check still fires for a lone minus.
- **SELECT grammar**: projection list accepts
  `col`, `col -> 'key'`, and `col ->> 'key'`. Key is a
  single-quoted text literal (same rule as any Value
  Text literal).
- **Evaluation**:
  - Source cell `Value::Json(s)` or `Value::String(s)`
    (legacy pre-ADR-0173 JSON row) is parsed via
    `serde_json`; non-JSON cell yields
    `SqlError::Type`.
  - Missing key → `Value::Null`.
  - Source `NULL` → `Value::Null`.
  - `->` returns the sub-value serialized as JSON
    (`Value::Json`).
  - `->>` returns the textual form (`Value::String`):
    JSON strings are unwrapped (no surrounding quotes);
    numbers, booleans, arrays, objects use serde_json's
    canonical rendering.
- **Output column names**: `col->'key'` and
  `col->>'key'` (operator + quoted key for
  readability).

## 2. Out of scope

- **Array index**. `col -> 0` is not supported; keys
  must be text literals. A numeric key is rejected at
  parse with `SqlError::Syntax`.
- **Chained paths**. `col -> 'a' -> 'b'` would require
  a parse-side left-fold; deferred.
- **JSON path in WHERE predicates**. WHERE still only
  accepts bare-column `=` equality on PK columns (the
  ADR 0112 range-rejection rule). Extending WHERE
  would collide with that boundary and needs its own
  ADR.
- **`DomainSqlEngine`, `HistorySqlEngine`,
  `MvccSqlEngine` SELECT**. Deferred mirror slice.
- **`UPDATE SET col -> 'k' = <expr>`** — JSON mutation
  is not a supported write path.
- **Named path containers** (`#>`, `#>>` with path
  arrays). Deferred.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/json_path_sql.rs`
(8 tests, all green):

1. `arrow_extracts_json_subvalue_preserving_json_variant`
   — `body -> 'nested'` returns `Value::Json`.
2. `arrow_arrow_extracts_as_text` — `body ->> 'name'`
   returns `Value::String` with the string unwrapped.
3. `arrow_arrow_on_numeric_subvalue_renders_canonical_text`
   — `body ->> 'level'` (an integer) returns the
   canonical `"3"`.
4. `missing_key_returns_null`.
5. `path_combines_with_other_projections_and_scan` —
   mixed `SELECT id, body ->> 'name' FROM ...` works
   under a full scan.
6. `path_on_non_json_column_fails_cleanly` — a TEXT
   column whose stored content is not JSON raises
   `SqlError::Type`.
7. `path_rejects_non_text_key_literal` — integer key
   is a parse error.
8. `path_cell_null_passes_through_as_null`.

Clippy clean on `-D warnings`. Earlier range-rejection
corpus (`rejects_join_aggregate_and_or_and_range_and_alias`)
continues to pass — the new tokens live outside the
WHERE grammar.

## 4. Roadmap impact

- ADR 0113 §2 "JSON path operators" follow-up closed
  at minimum scope (SELECT projection on `SqlEngine`).
- ADR 0173 §2 cross-reference closed.
- BRD-REL-004 JSON reach: `Value::Json` round-trip
  (ADR 0173) + path extraction in SELECT (ADR 0186).
- Remaining BRD-REL-004 work: WHERE-side path, chained
  paths, array index, JSON mutation, and the three
  other SQL engines.
