# ADR 0211 — ESCAPE clause for LIKE / ILIKE

## Status

Accepted.

## Context

ADRs 0209 and 0210 added `LIKE` and `ILIKE` with SQL-standard `%`
and `_` wildcards but no way to match a literal `%` or `_` in the
data. SQL's `ESCAPE 'c'` clause solves this by designating a
character that quotes the next literal pattern character.

## Decision

Extend the `Like` / `NotLike` / `Ilike` / `NotIlike` matcher
variants with an `escape: Option<char>` and parse an optional
`ESCAPE '<char>'` suffix.

- Parser (`parse_optional_escape`) runs immediately after the
  pattern literal. The escape literal must be a single-character
  text (`SqlError::Syntax` otherwise); empty and multi-character
  strings are rejected.
- `like_matches` grows the `escape: Option<char>` parameter. When
  the pattern head matches the escape char, the next pattern char
  is taken as a literal:
  - `\% ` → literal `%`
  - `\_ ` → literal `_`
  - `\\ ` → literal `\`
- Dangling escape at end-of-pattern consumes the escape but has no
  next char to match — the predicate returns false (not a parse
  error, matching SQL's permissive rules here).
- For `ILIKE`, the escape character is lowercased alongside the
  pattern so it still matches the lowercased extracted text.

## Consequences

- No engine-side changes — all four engines carry it through
  `apply_where_filter`. Composes with every earlier matcher and
  combinator.
- `like_matches`'s public signature changed to accept `escape`;
  existing call sites updated in-crate.
- Validated by `tests/json_path_where_like_escape.rs` (12 cases:
  literal `%`, `_`, `\` escaped; without ESCAPE wildcards still
  active; single-char and empty escape rejection; NOT LIKE /
  ILIKE / NOT ILIKE with escape; dangling escape at EOL; custom
  escape character; plain LIKE unchanged). The 17 LIKE and 12
  ILIKE tests still pass.
