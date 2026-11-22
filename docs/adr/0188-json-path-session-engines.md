# ADR 0188 — JSON path operators across session engines (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-22.** Mirrors ADR 0186 JSON path
operators (`->`, `->>`) from `SqlEngine` SELECT into
`DomainSqlEngine`, `HistorySqlEngine`, and `MvccSqlEngine`.

## 1. Scope

- Each session engine's SELECT branch now parses projections
  via the shared `sql::parse_projection` helper (ADR 0186)
  and applies `sql::apply_projection` per cell after the
  base row projection. Output column labels use the shared
  `Projection::label()` renderer.
- Lexer tweak (ADR 0186 extension): `Token::Arrow` /
  `Token::ArrowText` are now emitted in expression mode as
  well, because `HistorySqlEngine` lexes with
  `Parser::new_expressions`. The expression grammar
  doesn't bind these tokens, so a stray `->` inside an
  UPDATE RHS would raise a parse error rather than silently
  lexing as `-` then `>`.
- HistorySqlEngine SELECT with ORDER BY/LIMIT still runs
  the base `project_rows_with_control` step first (budget
  + control honored), then applies JSON path extraction on
  the projected cells.

## 2. Out of scope

- **WHERE-side JSON path**. Same honest boundary as
  ADR 0186.
- **Chained paths** (`col -> 'a' -> 'b'`), array indices,
  JSON mutation on UPDATE — all deferred.
- **`#>` / `#>>` container-path operators** — deferred.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/json_path_session_engines.rs`
(6 tests, all green):

DomainSqlEngine (2):
1. `domain_arrow_extracts_json_subvalue` — `->>` returns
   the text form.
2. `domain_arrow_returns_json_subvalue` — `->` returns
   `Value::Json`.

HistorySqlEngine (2):
3. `history_arrow_extracts_json_subvalue`.
4. `history_arrow_combines_with_order_by` — proves the
   projection runs after `SelectWindow::apply` sorting.

MvccSqlEngine (2):
5. `mvcc_arrow_extracts_json_subvalue`.
6. `mvcc_arrow_missing_key_returns_null`.

ADR 0186 SqlEngine suite (8 tests) continues to pass.
`sql_subset_corpus` (46 tests) continues to pass — the
expression-mode lexer tweak did not break any UPDATE
expression grammar rule. Clippy clean on `-D warnings`.

## 4. Roadmap impact

BRD-REL-004 JSON path operator coverage after this ADR:

| Engine | SELECT `->` / `->>` |
|---|---|
| `SqlEngine` | ADR 0186 |
| `DomainSqlEngine` | **ADR 0188** |
| `HistorySqlEngine` | **ADR 0188** |
| `MvccSqlEngine` | **ADR 0188** |

Combined with the ADR 0187 FK-session-engines mirror,
every BRD-REL-002 constraint path (CHECK / UNIQUE / FK)
and every BRD-REL-004 JSON surface now behaves
identically across all four SQL engines at Gate 1
scope. WHERE-side JSON path, chained paths, array
index, and JSON mutation remain the next category of
work.
