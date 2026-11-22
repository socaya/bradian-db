# ADR 0210 — ILIKE / NOT ILIKE (case-insensitive LIKE)

## Status

Accepted.

## Context

ADR 0209 added case-sensitive `LIKE`. Real-world string queries
very often need case-insensitive matching (user handles, product
titles, email addresses). PostgreSQL's `ILIKE` is the idiomatic
spelling.

## Decision

Add `JsonMatcher::Ilike { pattern_lower }` and `NotIlike { … }`.

- Parser accepts `ILIKE '<text>'` and `NOT ILIKE '<text>'` after a
  JSON path LHS. Pattern must be a quoted text literal (same
  rule as LIKE).
- The parser pre-lowercases the stored pattern using
  `to_ascii_lowercase` so matching per-row only has to lowercase
  the extracted value.
- Matching reuses `like_matches` — the `%`, `_`, and literal-char
  rules are identical.
- ASCII case folding only. Non-ASCII characters are passed through
  unchanged; locale-aware case folding would require an external
  library and is deliberately out of scope for Gate 1.

## Consequences

- No engine-side changes — all four engines carry it through
  `apply_where_filter`. Composes with every earlier matcher.
- No ESCAPE clause (same as LIKE).
- Validated by `tests/json_path_where_ilike.rs` (12 cases: mixed-
  case / uppercase pattern / underscore, NOT ILIKE, arrow-JSON-
  string unwrapping, AND / OR composition, non-text-pattern
  rejection, UPDATE / DELETE, Domain + MVCC parity). The 17 LIKE
  tests still pass.
