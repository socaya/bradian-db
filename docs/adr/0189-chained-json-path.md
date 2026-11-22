# ADR 0189 — Chained JSON path segments in SELECT (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-22.** Follow-up to ADR 0186 /
ADR 0188. Closes the "chained paths" open item noted in ADR
0186 §2 and ADR 0188 §2.

## 1. Scope

- `Projection::JsonPath` generalized from a single `key`
  field to `path: Vec<String>` (ordered segment list) and
  one `as_text` flag applied to the final step.
- `parse_projection` loops over Arrow / ArrowText + Text
  pairs after the bare column identifier. Each `->`
  segment appends a key; a terminating `->>` sets
  `as_text = true` and locks the path (any further `->` /
  `->>` after a `->>` is rejected with
  `SqlError::Unsupported` — text extraction terminates
  the chain).
- `apply_projection` walks each segment in order,
  short-circuiting to `Value::Null` on any missing /
  JSON-null step. Only the final rendering is governed by
  `as_text`.
- `Projection::label()` renders each intermediate segment
  as `->'key'` and the final one as `->'key'` or
  `->>'key'` for human-readable output column names
  (e.g. `body->'a'->'b'->>'c'`).

All four SQL engines pick up chained paths automatically
since they call the same `parse_projection` /
`apply_projection` helpers (via ADR 0186 for SqlEngine and
ADR 0188 for the three session engines).

## 2. Out of scope

- **Mixing `->>` mid-chain**. `->>` returns text, and
  re-interpreting a TEXT cell as JSON for further
  navigation would require another parse. Rejected with
  `SqlError::Unsupported` at parse time.
- **Array index**. `col -> 0` still unsupported; keys
  must be text literals.
- **WHERE-side JSON path** — follow-up ADR (0190).
- **`#>` / `#>>` container-path operators**.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/json_path_chained.rs`
(10 tests, all green):

1. `sql_engine_chained_arrow_returns_json` — two-segment
   chain, output is `Value::Json`; label is
   `body->'a'->'b'`.
2. `sql_engine_chained_terminating_arrow_arrow_returns_text`
   — three-segment chain ending in `->>`; output is the
   text `"deep"`.
3. `sql_engine_chained_arrow_arrow_on_numeric_subvalue_renders_canonical_text`
   — `->>` on a JSON number renders the canonical
   decimal string.
4. `sql_engine_chained_path_short_circuits_on_null_midway`.
5. `sql_engine_chained_path_missing_key_returns_null`.
6. `chained_arrow_arrow_followed_by_more_segments_is_rejected`
   — parser rejects `body ->> 'a' -> 'b'`.
7. `chained_path_mixes_with_bare_column_projection` —
   `SELECT id, body -> 'a' ->> 'n'`.
8. `domain_engine_chained_path`.
9. `history_engine_chained_path`.
10. `mvcc_engine_chained_path`.

ADR 0186 and ADR 0188 suites (8 + 6 tests) continue to
pass unchanged — single-segment paths remain their special
case. Clippy clean on `-D warnings`.

## 4. Roadmap impact

BRD-REL-004 JSON surface after this ADR:

| Capability | Status |
|---|---|
| `Value::Json` round-trip (ADR 0173) | ✓ all four engines |
| SELECT single-segment `->` / `->>` (ADR 0186 + 0188) | ✓ all four engines |
| SELECT chained `-> 'a' -> 'b' ->> 'c'` (**ADR 0189**) | ✓ all four engines |
| WHERE-side JSON path | open — ADR 0190 |
| Array index (`col -> 0`) | open |
| JSON mutation on UPDATE | open |
| `#>` / `#>>` container-path operators | open |
