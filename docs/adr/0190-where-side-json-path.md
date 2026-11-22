# ADR 0190 — WHERE-side JSON path equality predicates (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-22.** Follow-up to ADR 0186 /
ADR 0188 / ADR 0189. Closes the WHERE-side JSON path
follow-up noted in ADR 0186 §2 and ADR 0188 §2 across all
four SQL engines.

## 1. Scope

- **Grammar**: a SELECT's WHERE clause can now carry one
  of two shapes:
  - `WHERE col = lit [AND ...]` — the existing PK
    equality path. Preserves `tx.get` / `store.get`
    fast lookup.
  - `WHERE col -> 'k' [...] = lit` or
    `WHERE col ->> 'k' [...] = lit` — new JSON-path
    equality predicate. Full-scan + per-row filter.
- The grammar dispatcher (`parse_select_where` in
  `sql.rs`) uses a one-token lookahead: after the first
  column word, if the next token is `Arrow` /
  `ArrowText`, it builds a `SelectWhere::JsonPathEquality`;
  otherwise it rewinds and parses the existing PK
  predicate list.
- JSON-path WHERE accepts chained segments (ADR 0189) and
  allows the final `->>` text-terminator form.
- Equality semantics:
  - `->>` compares against **TEXT**. Literal must be a
    quoted string (or a numeric/boolean literal, both
    rendered as their canonical text).
  - `->` compares against **JSON** (bytewise against the
    sub-value's canonical JSON serialization). A
    quoted string literal becomes a JSON string; a
    numeric literal becomes a JSON number.
- `NULL` on either side never matches (SQL three-valued
  logic — equality with `NULL` is UNKNOWN).

Call-site wiring:
- `SqlEngine::execute_impl` SELECT.
- `DomainSqlEngine` SELECT branch.
- `HistorySqlEngine` SELECT branch (including AS OF,
  which combines with a JSON-path WHERE as a scan-at-LSN
  + post-filter).
- `MvccSqlEngine` SELECT branch.

## 2. Out of scope

- **JSON path in UPDATE / DELETE WHERE**. Both still
  require PK equality (and HistorySqlEngine's UPDATE
  grammar uses the sql_expression evaluator which is
  numeric-only).
- **Non-equality comparisons** (`<`, `>`, `<=`, `>=`,
  `<>`) against JSON path. Honest boundary — ADR 0112
  range-rejection still stands; this slice is strict
  equality.
- **AND combinators between a JSON path and a PK
  predicate**. The grammar is a single predicate.
- **Array index**, `#>` / `#>>` container paths — same
  honest boundary as ADR 0186 / 0189.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/json_path_where.rs`
(9 tests, all green):

SqlEngine (6):
1. `where_arrow_arrow_text_equality` — `WHERE body
   ->> 'name' = 'bob'`.
2. `where_arrow_arrow_matches_numeric_subvalue_as_text`
   — `WHERE body ->> 'level' = '3'`.
3. `where_missing_key_never_matches` — rows lacking the
   key are filtered out.
4. `where_combines_with_json_path_projection` — path on
   both sides of SELECT.
5. `where_chained_arrow_path_filter` — `WHERE body ->
   'a' ->> 'b' = 'x'`.
6. `where_pk_equality_still_works` — regression proof
   that the dispatcher does NOT regress PK equality.

Session engines (3):
7. `domain_where_json_path_filter`.
8. `history_where_json_path_filter`.
9. `mvcc_where_json_path_filter`.

ADR 0112 range-rejection corpus (46 tests) continues to
pass. ADR 0186 / 0188 / 0189 JSON path suites continue
to pass. Clippy clean on `-D warnings`.

## 4. Roadmap impact

BRD-REL-004 JSON surface coverage after this ADR:

| Capability | Status |
|---|---|
| `Value::Json` round-trip (ADR 0173) | ✓ all four engines |
| SELECT single-segment `->` / `->>` | ✓ ADR 0186 + 0188 |
| SELECT chained segments | ✓ ADR 0189 |
| WHERE-side equality (single or chained) | ✓ **ADR 0190** |
| Non-equality / range on JSON path | open |
| Array index (`col -> 0`) | open |
| JSON mutation on UPDATE | open |
| `#>` / `#>>` container operators | open |
| JSON path in UPDATE / DELETE WHERE | open |
