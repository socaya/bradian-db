# ADR 0293 — Commit-time column projection (receipt)

## Status

Accepted. Second slice of P1-1.

## Context

ADR 0292 shipped a parity path that scans RowStore rows and
runs them through `bradian_runtime::RecordBatchBuilder` +
`parallel_filter_aggregate`. The next piece of P1-1 is
column projection "after COMMIT in a shared-domain SQL
commit" — the HTAP engine needs a durable, synchronous
hand-off from the transactional commit path to the columnar
side so segments and incremental views can refresh without
re-scanning the whole table.

`MultiTableTransaction::commit` currently consumes its
staged changes and returns only the WAL LSN. Column
projection has to re-scan if it wants the committed rows;
that is both wasteful and racy against a subsequent commit.

## Decision

Add an additive API:

- `MultiTableTransaction::commit_with_receipt(self) ->
  DomainResult<CommitReceipt>` returns the WAL LSN and the
  exact post-image rows that were just persisted, grouped
  by table.
- `CommitReceipt { pub lsn: u64, pub upserts_by_table:
  BTreeMap<TableId, Vec<Vec<Value>>> }`.
- The existing `commit(self) -> DomainResult<u64>`
  delegates to `commit_with_receipt`, drops the receipt
  and returns the LSN. All existing callers are unchanged.
- Deletes are not projected in this slice (columnar
  tombstones and incremental view retraction land in
  ADR 0294). Document the gap explicitly.

Scope limits:

- The receipt is in-memory only. No new on-disk structure,
  no new WAL record kind.
- Receipts are per-commit; the domain does not retain a
  log of receipts across commits.
- Row-level MVCC retention / historical projection is out
  of scope: history already has its own path (G22).
- Not plumbed through `DomainSqlEngine::execute` yet. The
  shared-domain SQL commit still returns `SqlResultSet`,
  which has no slot for a receipt; wiring it through is a
  separate slice once a consumer is in place. The native
  `MultiTableTransaction` entrypoint is enough to prove
  commit-time column projection.

## Consequences

- A columnar consumer (segment writer, view maintainer)
  can subscribe to commit-time rows synchronously without
  re-reading the shared store.
- The receipt grows with the commit size, bounded by the
  transaction's existing `self.changes` admission limits.
  It is dropped immediately in the receiptless path.
- 1 integration test
  (`tests/commit_column_projection.rs`) exercises the new
  entrypoint against a two-table shared domain: inserts,
  replaces and a delete across both tables, then
  reconstructs `RecordBatch`es from the upserts-by-table
  map via the ADR 0292 adapter and asserts parity with
  the stored post-image rows.
- No behaviour change for `commit` callers. No format
  change. No clippy regressions.
