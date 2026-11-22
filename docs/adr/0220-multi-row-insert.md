# ADR 0220 — Multi-row INSERT VALUES

## Status

Accepted.

## Context

Every engine currently accepts a single VALUES tuple per
`INSERT`. Bulk inserts (loading, tests, migrations) have to run
one statement per row, which multiplies parse + validate +
WAL-charge overhead.

## Decision

Accept `INSERT INTO t (cols) VALUES (…), (…), (…)` as one
statement. Each tuple is parsed into a `Vec<Literal>` using the
existing `Parser::list` (or `Parser::insert_literal` list for
the historical engine so `DEFAULT` keywords keep working), and
the parser loops on the comma separator between tuples.

Semantics:
- Column list is parsed once and applies to every tuple.
- Every tuple must match the column list in count. The column
  list must still cover all table columns (same rule as the
  single-tuple INSERT; column reordering remains allowed).
- Rows are **coerced** before any storage I/O: a bad literal in
  tuple #17 fails the whole batch cleanly with no partial writes
  — matching single-statement expectations.
- Budget admission runs as a batch preflight
  (`preview_charge(rows.len(), batch_bytes)`).
- CHECK, FK, and UNIQUE run per row, in order. UNIQUE sees
  earlier rows in the same batch via a local
  `unique_pool: Vec<Vec<Value>>` prepopulated from the live
  table scan.
- For the three transactional engines, FK runs through the
  active transaction so both the batch's earlier rows and the
  session's other staged mutations are visible.
- Non-atomicity on PK / UNIQUE after some rows already landed
  mirrors the engines' existing single-row INSERT semantics for
  the base engine; transactional engines roll the whole batch
  back together on COMMIT abort.

## Consequences

- `affected` in the `SqlResultSet::Command` is now the count of
  rows that actually reached storage, not always `1`.
- Base engine is still non-atomic within a batch (no transaction
  wrapper): a batch that fails on row K leaves rows 1..K-1
  committed. Callers that need batch atomicity should use the
  transactional engines (`DomainSqlEngine`, `MvccSqlEngine`,
  `HistorySqlEngine`) and COMMIT the whole statement.
- No `VALUES (…) SELECT …` shape yet; no `ON CONFLICT`; no
  `RETURNING` clause — all deferred.
- Validated by `tests/base_multirow_insert.rs` (10 cases:
  single-tuple back-compat, two tuples, five tuples, bad-type
  mid-batch rejection, mismatched tuple width rejection,
  UNIQUE-within-batch, PK collision within batch, CHECK
  constraint, Domain + MVCC parity).
