# ADR 0274 — WITH / CTE (non-recursive, narrow scope)

## Status

Accepted.

## Context

The autonomous G1 run deferred CTE because a general
implementation needs a per-session transient table registry.
A narrower shape still provides real value: single-CTE
non-recursive bindings where the outer references the CTE
once as a FROM target. This ADR ships that.

## Decision

- New `resolve_with_cte(sql)` runs before every other pass
  in `execute_impl`. Detects a `WITH <name> AS (<inner>) <outer>`
  prefix and rewrites it:
  - If `<outer>` is `SELECT * FROM <name>` → return `<inner>`
    verbatim.
  - If `<outer>` is `SELECT <proj> FROM <name> [tail]` AND
    `<inner>` is `SELECT * FROM <inner_table>` → rewrite to
    `SELECT <proj> FROM <inner_table> [tail]`.
  - Anything else surfaces `SqlError::Unsupported` with a
    diagnostic.
- `split_identifier(s)` helper extracts a leading
  identifier.

## Consequences

- Scope limits:
  - **Single CTE only** — `WITH a AS …, b AS …` is
    unsupported.
  - **Non-recursive only** — `WITH RECURSIVE …` rejected
    (standard SQL optional feature).
  - Outer must reference the CTE as a FROM target, not in a
    subquery.
  - Projection narrowing requires the inner to be
    `SELECT * FROM <table>` — a general projection rewrite
    is a later slice.
- Unbalanced parens, missing AS, missing FROM all surface
  explicit `SqlError::Syntax` / `SqlError::Unsupported`.
- 7 cases in `tests/with_cte.rs` cover: `SELECT *` passthrough;
  outer WHERE narrows; name-mismatch rejected; projection
  narrowing; missing AS rejected; unbalanced parens
  rejected; outer without FROM rejected.
