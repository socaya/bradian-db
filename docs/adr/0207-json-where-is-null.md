# ADR 0207 — IS NULL / IS NOT NULL on JSON path WHERE

## Status

Accepted.

## Context

The JSON-path WHERE evaluator (ADRs 0190 / 0195 / 0204 / 0205 /
0206) short-circuits every predicate to false when the extracted
value is `NULL`. That matches SQL three-valued logic for scalar
comparisons and `IN`, but makes it impossible to filter for
missing or explicitly-null JSON sub-values. Callers need the
standard `IS NULL` / `IS NOT NULL` tests.

## Decision

Add `JsonMatcher::IsNull` and `JsonMatcher::IsNotNull`. Evaluation
in `apply_where_filter` branches BEFORE the NULL-extracted guard,
so these matchers see and act on `Value::Null`.

- Parser: after a JSON path LHS, try `IS` first; the optional
  `NOT` here belongs to the IS test (so `IS NULL` and `IS NOT
  NULL` are the only accepted IS-forms). Bare `IS` without `NULL`
  is rejected (`SqlError::Unsupported`). `NOT IN` continues to
  parse on the separate `NOT …` branch.
- `IsNull`: matches when `apply_projection` yields `Value::Null`
  (missing key or an explicit `null` in the JSON).
- `IsNotNull`: inverse.
- Composes with ADR 0204/0205 AND/OR DNF and ADR 0206 IN/NOT IN
  unchanged.

## Consequences

- All four engines pick this up through `apply_where_filter`
  without engine-side changes.
- `->>` with `IS NULL` behaves the same as `->` — both return
  `Value::Null` when the key is absent.
- Validated by `tests/json_path_where_is_null.rs` (12 cases:
  missing vs explicit JSON null, `->>` form, AND / OR composition,
  combined with NOT IN, container form, bare-IS rejection,
  UPDATE / DELETE, Domain + MVCC parity).
- No `IS TRUE` / `IS FALSE` / `IS DISTINCT FROM` yet — future
  extensions.
