# ADR 0209 — LIKE / NOT LIKE on JSON path WHERE

## Status

Accepted.

## Context

The JSON-path WHERE matcher set now covers equality / inequality
(ADR 0190/0195), AND/OR DNF (ADRs 0204/0205), membership (ADR 0206),
NULL (ADR 0207), and ranges (ADR 0208). It still cannot test text
patterns, which is one of the most common ad-hoc query shapes
(`LIKE 'prefix%'`).

## Decision

Add `JsonMatcher::Like { pattern }` and `NotLike { pattern }` with
SQL-standard pattern semantics.

- Parser accepts `LIKE '<text>'` and `NOT LIKE '<text>'` after a
  JSON path LHS. Pattern must be a single-quoted text literal;
  numeric or other literal forms are rejected at parse.
- `like_matches` compiles nothing — it walks the pattern and text
  recursively with `%` collapsing and bounded backtracking. `%`
  matches zero or more characters; `_` matches exactly one
  character. Everything else is a literal case-sensitive match.
- `like_source` unwraps the extracted value:
  - `Value::String(s)` → use `s` directly.
  - `Value::Json(j)` where `j` is a JSON string scalar → unwrap
    via `serde_json::from_str` so `-> 'name'` behaves the same as
    `->> 'name'`.
  - Other variants → predicate evaluates to false (numeric,
    boolean, arrays, objects are not matchable with `LIKE`).
- No `ESCAPE` clause yet — literal `%` or `_` inside the data
  cannot be matched via `LIKE` today; future extension.

## Consequences

- Carries through all four engines via `apply_where_filter` without
  engine-side changes. Composes with AND / OR, IN / NOT IN,
  BETWEEN, and IS [NOT] NULL.
- Validated by `tests/json_path_where_like.rs` (17 cases: prefix,
  suffix, both-sides, underscore, mixed, case sensitivity, exact
  match, NOT LIKE, `->` arrow against a JSON string, non-string
  short-circuit, AND / OR composition, non-text-pattern rejection,
  UPDATE / DELETE, Domain + MVCC parity). The 10 cmp, 11 AND,
  12 OR, 14 IN, 12 IS NULL, and 14 BETWEEN tests still pass
  unchanged.
