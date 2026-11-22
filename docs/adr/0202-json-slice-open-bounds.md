# ADR 0202 — Open-ended JSON slices

## Status

Accepted.

## Context

ADR 0197 introduced `JsonSegment::Slice { start, end }` where both
bounds were required non-negative integers. Callers often want
`[:n]`, `[n:]`, or the whole array `[:]` as a shorthand for
"slice to the end" or "slice from the beginning." The required
grammar is `->` / `->>` / `#>` / `#>>` followed by a slice whose
`start` or `end` (or both) may be omitted.

## Decision

Make both slice bounds optional.

- `JsonSegment::Slice { start: Option<usize>, end: Option<usize> }`.
  `None` on `start` means "from zero"; `None` on `end` means
  "to `arr.len()`." Both-`None` is the whole array.
- Arrow-chain parser accepts a leading `Token::Symbol(':')` (opens
  with `None` start), and after a numeric start the end may be any
  non-numeric follower (opens with `None` end).
- Container-form parser (`#> '{…,start:end,…}'`) accepts an empty
  `start_s` or `end_s` after splitting on `:`.
- `apply_slice` resolves each `None` at runtime against the actual
  array length. Non-array sources still resolve to `NULL`.
- `start > end` is still rejected at parse when both are present.
- Slice mutation on UPDATE SET LHS remains out of scope (ADR 0197).
- Render preserves the user's open-ended form in labels.

## Consequences

- Two previously-rejecting parser tests (`slice_missing_end_rejected`,
  `slice_container_bad_form_rejected`) are superseded; the new
  valid-shape assertions live in those slots.
- New `tests/json_path_slice_open.rs` covers 13 open-ended cases
  (arrow + container, both bounds, chain, non-array, empty array,
  past-length clamp, UPDATE LHS rejection, container bad text
  segment).
- No change to engine semantics outside path resolution; no change
  to the WHERE / SELECT / UPDATE dispatchers.
