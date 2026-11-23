# ADR 0231 — HistorySqlSession INSERT SELECT

## Status

Accepted.

## Context

ADR 0230 added `INSERT INTO t (cols) SELECT …` to
`DomainSqlSession` and `MvccSqlSession` but deferred
`HistorySqlSession`. Completing the set means the historical
engine can now participate in bulk data flows.

## Decision

Mirror the Domain/Mvcc INSERT SELECT branch inside
`HistorySqlSession::execute_inner`. Source rows are read through
the active transaction (so session-staged writes are visible to
the copy); CHECK / FK / UNIQUE run per projected row; Int32 ↔
Int64 widen is inherited. RETURNING composes.

## Consequences

- The DEFAULT support that distinguishes HistorySqlEngine's
  single-row VALUES INSERT does not apply here: source columns
  carry real values, so the SELECT-driven path skips the
  DEFAULT-resolution branch entirely.
- No new tests beyond the ADR 0230 cross-engine coverage —
  HistorySqlSession was the only engine missing INSERT SELECT.
