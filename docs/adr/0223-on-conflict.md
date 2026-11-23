# ADR 0223 — INSERT … ON CONFLICT DO NOTHING / DO UPDATE

## Status

Accepted.

## Context

Upserts are the most common idiom for idempotent inserts and
"keep the latest" semantics. ADR 0220 added multi-row INSERT;
ADR 0221 added RETURNING. The missing primitive is a way to
resolve PK collisions without running two statements.

## Decision

Parse an optional `ON CONFLICT (pk_cols) DO NOTHING |
DO UPDATE SET col = lit[, …]` suffix after the VALUES list
(before RETURNING). The conflict target must be the primary-key
column set; UNIQUE-key targets are a future extension.

- `OnConflict::{ DoNothing, DoUpdate { assignments } }`.
- `parse_on_conflict` validates:
  - Target is exactly the PK set (order-free).
  - DoUpdate cannot touch PK columns.
  - DoUpdate assignments resolve to real columns at parse time.
- Per-row execution:
  - Resolve the candidate row's PK and probe the store.
  - No conflict → normal INSERT.
  - DoNothing + conflict → skip the row.
  - DoUpdate + conflict → apply assignments to the existing
    row, re-run CHECK / FK / UNIQUE, replace.
- `RETURNING` composes with ON CONFLICT: both branches
  (fresh insert, upsert-updated row) land in the capture list.
- All four engines inherit the clause.

## Consequences

- Scope cuts: literal RHS only (no `EXCLUDED.col`), no `WHERE`
  on the DO UPDATE action, no UNIQUE-targeted conflict, no
  cascading.
- 9 cases in `tests/base_on_conflict.rs` cover DO NOTHING
  skip-vs-insert, DO UPDATE mutate, mixed hit/miss, RETURNING
  composition, PK-column UPDATE rejection, non-PK target
  rejection, Domain + MVCC parity.
