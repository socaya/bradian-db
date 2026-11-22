# Master Gate 1 — JSON path operator verification

22 November 2026. ADR 0186. Closes the ADR 0113 §2 /
ADR 0173 §2 "JSON path operators" follow-up at minimum
scope (SELECT on `SqlEngine`).

## Executed result

| Executed check | Result |
|---|---|
| `cargo test --locked -p bradian-relational --test json_path_sql` | 8 passed, 0 failed |
| `cargo test --locked -p bradian-relational --test sql_subset_corpus rejects_join` | 1 passed (range-rejection preserved) |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## Finding and action

ADR 0173 added `Value::Json` round-trip but no way to
extract a sub-value from SQL. ADR 0113 §2 and ADR 0173
§2 both deferred the standard `->` / `->>` operators.

Change (ADR 0186):

- Lexer: new `Token::Arrow` (`->`) and
  `Token::ArrowText` (`->>`). Only emitted outside
  expression mode so the "`-` must prefix a numeric
  literal" rule still applies for a lone minus.
- `Parser::advance()` helper — move the cursor forward
  after a `peek`.
- New `Projection` enum and `parse_projection` +
  `apply_projection` helpers in `sql.rs`.
- `SqlEngine::execute_impl` SELECT branch now parses
  projections via `parse_projection`, routes through
  `apply_projection` after the base
  `project_rows_with_control` step, and emits
  human-readable labels (`col`, `col->'key'`,
  `col->>'key'`).
- `->` returns `Value::Json` of the sub-value; `->>`
  unwraps JSON strings and renders other scalar/
  compound shapes via `serde_json`.
- Missing key / NULL cell → `Value::Null`. Non-JSON
  cell content → `SqlError::Type`. Numeric key → parse
  error.

Added tests
(`crates/bradian-relational/tests/json_path_sql.rs`,
8 total): arrow preserves JSON, double-arrow returns
text (strings unwrapped), numeric sub-value renders
canonical text, missing key, mixed projection under
scan, non-JSON column type error, numeric key parse
error, NULL cell pass-through.

Existing ADR 0112 range-rejection corpus still passes
— the new tokens live outside the WHERE grammar.

## Scope limits (honest)

- `SqlEngine` SELECT only. DomainSqlEngine /
  HistorySqlEngine / MvccSqlEngine SELECT and any
  WHERE-side path support remain explicitly open.
- Keys are string literals only; no array index.
- No chained paths (`col -> 'a' -> 'b'`).
- No JSON mutation on UPDATE.
- No `#>` / `#>>` container-path operators.

## Evidence

- Source:
  `crates/bradian-relational/src/sql.rs` — new tokens
  in the lexer, `Projection` enum, parse / apply
  helpers, SELECT branch refactor.
- Tests:
  `crates/bradian-relational/tests/json_path_sql.rs`.
- ADR: `docs/adr/0186-json-path-operators.md`.
