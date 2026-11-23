# ADR 0230 — UNION and INSERT SELECT in transactional engines

## Status

Accepted.

## Context

ADRs 0224–0225 added UNION / UNION ALL and `INSERT INTO t
(cols) SELECT …` on the base `SqlEngine`. The three
transactional engines (`DomainSqlSession`, `MvccSqlSession`,
`HistorySqlSession`) still rejected both shapes.

## Decision

- Each session's `execute` entry point now splits top-level
  `UNION [ALL]` boundaries via `split_top_level_unions` and
  runs each arm recursively through the same `execute`,
  combining results with `dedup_rows` + `sort_distinct_output`
  for UNION or plain concatenation for UNION ALL. The session's
  mutation state is preserved across arms.
- `DomainSqlSession` and `MvccSqlSession` grow a narrow
  `INSERT INTO t (cols) SELECT cols FROM src [WHERE …]
  [RETURNING …]` branch in INSERT, reading source rows through
  the active transaction so a session's own staged inserts are
  visible to the copy. Type widening `Int32 ↔ Int64` matches
  ADR 0224. CHECK / FK / UNIQUE run per projected row.
- `HistorySqlSession` retains the VALUES-only INSERT shape in
  this slice — its `insert_literal` + DEFAULT handling
  deserves a dedicated adapter.

## Consequences

- No multi-SELECT composition on DML (`INSERT … VALUES …
  UNION …`) — each DML statement still produces one result.
- 5 cases in `tests/tx_union_insert_select.rs` cover
  DomainSqlEngine UNION, DomainSqlEngine INSERT SELECT with a
  separate COMMIT pair, DomainSqlEngine INSERT SELECT seeing
  the session's own staged writes, MvccSqlEngine UNION ALL,
  MvccSqlEngine INSERT SELECT with WHERE. The base engine's
  7 INSERT SELECT and 9 UNION tests still pass.
