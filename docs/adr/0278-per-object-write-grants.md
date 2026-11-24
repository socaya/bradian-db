# ADR 0278 — Per-object INSERT / UPDATE / DELETE grants

## Status

Accepted.

## Context

ADR 0258 landed per-object SELECT grants. Writes remained
global-only: users without `Capability::Write` couldn't be
granted INSERT, UPDATE, or DELETE on a specific table.

## Decision

- `ObjectCapability` gains `Insert`, `Update`, `Delete`
  variants alongside `Select`.
- `UserRecord::allows_on_table` extends the "global implies
  per-object" rule: `Capability::Write` implies Insert /
  Update / Delete on every table.
- SQL grammar: `GRANT { SELECT | INSERT | UPDATE | DELETE }
  ON <qualified-table> TO <user>` (and REVOKE). A new
  `detect_object_capability` helper replaces the hardcoded
  SELECT dispatch so adding more verbs is a one-line change.
- `AuthenticatedSession::execute` extracts the statement's
  verb and target via `classify_single_table_statement`.
  For INSERT / UPDATE / DELETE lacking global Write, if the
  user holds the matching per-object capability on the
  target table, the context is elevated with Write for the
  single call.

## Consequences

- Scope: single-table statements only. Multi-table SELECT
  (joins / subqueries) with mixed-ownership permissions is
  ADR 0280 territory.
- INSERT INTO ... SELECT ... FROM ... needs grants on both
  the target (Insert) and the source (Select); the current
  single-statement elevation picks the WRITE verb based on
  INSERT. A future slice can elevate both.
- 8 cases in `tests/per_object_write_grants.rs` cover:
  no-grant blocks INSERT; GRANT INSERT allows INSERT;
  INSERT grant does NOT enable UPDATE; GRANT UPDATE allows
  UPDATE; GRANT DELETE allows DELETE; REVOKE on next
  session; admin bypasses; snapshot reflects grants.
