# ADR 0201 — JSON array INSERT (splice) on UPDATE SET

## Status

Accepted.

## Context

ADR 0192 added `UPDATE … SET body -> 'k' = literal` for in-place JSON
sub-value replacement. ADR 0200 added `… REMOVE` for deletion. Callers
also need to *splice* new elements into an existing JSON array at a
specific position without rewriting the whole column. The required
grammar is `UPDATE … SET <col> <path> -> <index> INSERT <literal>`,
where the terminal segment is an array index.

## Decision

Introduce an `INSERT` terminal verb on JSON path SET elements across
all three online engines (`SqlEngine`, `DomainSqlEngine`,
`MvccSqlEngine`) and the historical `HistorySqlEngine`.

- Parser (`parse_projection_tail` + `mixed_assignments`) treats
  `INSERT <literal>` symmetrically with `REMOVE` and with
  `= <literal>`.
- `SetAssignment::JsonInsert { column, path, as_text, value }` carries
  the parsed form through planning;
  `HistoryAssignment::JsonInsert { … }` mirrors it on the historical
  engine.
- `apply_json_path_insert` + `descend_and_insert` walk the chained
  path (auto-creating intermediate objects as the other JSON path
  mutations already do), then at the terminal require an array and
  insert at position `n` with `0 ≤ n ≤ arr.len()`, shifting right.
- Non-array terminals, `IndexFromEnd`, and `Slice` terminals are
  rejected with `SqlError::Unsupported`. Out-of-range indices are
  rejected with `SqlError::Type`.
- `->` writes the literal under its native JSON tag; `->>` forces a
  string tag (consistent with ADR 0192).
- INSERT ordering in UPDATE execution: column assignments → JSON
  path replacements → JSON removes → JSON inserts. CHECK, FK, and
  UNIQUE validation run against the final mutated row.

## Consequences

- Append is `-> arr.len()`; prepend is `-> 0`; splice is `-> k`.
- No concat (`||`), no multi-element splice, no negative-index
  insert — deliberately out of scope.
- Carried through all three online engines and the historical
  engine; validated by `tests/json_path_insert.rs` (13 cases) and
  the three new history cases in
  `tests/json_path_mutation_history.rs`.
