# ADR 0294 — Incremental view maintenance from commit receipts

## Status

Accepted. Third slice of P1-1.

## Context

ADR 0293 shipped a `CommitReceipt` carrying per-table
upsert post-images. For a materialised view to refresh
incrementally after a commit it also needs the **pre-image**
of each row that was replaced or deleted — otherwise a SUM
or COUNT view cannot retract the stale contribution of a
key before applying the new one.

`bradian-views::incremental::IncrementalAggregate` already
knows how to retract/apply additive aggregates (`Sum`,
`Count`, `Avg`) given `(old_value, new_value)` pairs per
group key. It did not have a feed.

## Decision

1. Enrich `CommitReceipt` with a richer per-table change
   log, side-by-side with the existing upserts map so no
   current consumer breaks:

   ```rust
   pub struct CommitReceipt {
       pub lsn: u64,
       pub upserts_by_table: BTreeMap<TableId, Vec<Vec<Value>>>,
       pub changes_by_table: BTreeMap<TableId, Vec<CommitRowChange>>,
   }
   pub enum CommitRowChange {
       Insert { new: Vec<Value> },
       Update { old: Vec<Value>, new: Vec<Value> },
       Delete { old: Vec<Value> },
   }
   ```

   The `old` images are the transaction's pre-commit
   snapshot values — the same ones `MultiTableTransaction`
   already holds in `self.snapshot` to detect duplicate
   keys and missing targets. No extra read.

2. Integration test
   `tests/incremental_view_refresh.rs` demonstrates
   feeding commit receipts into an
   `IncrementalAggregate::Sum` keyed by a non-PK
   grouping column across a mixed stream of inserts,
   replaces and deletes, and asserts the running
   aggregate matches a fresh scan-based reaggregation
   after each commit.

3. Honest scope:
   - Still no `DomainSqlEngine` wiring — the shared SQL
     frontend returns `SqlResultSet`, which does not
     expose a receipt channel yet. That slice is
     separate once a production consumer exists.
   - Non-invertible aggregates (`Min` / `Max` /
     `CountDistinct`) still require a full recompute on
     dirty keys; the receipt's change log is sufficient
     for that (it names the affected keys), but this
     slice only exercises the invertible path.
   - No on-disk receipt log; a crash between commit
     return and consumer apply loses the receipt. A
     durable change-log is out of scope.
   - Multiple consumers of the same receipt are the
     caller's responsibility (receipt is `Clone`).

## Consequences

- An `IncrementalAggregate` can run as a synchronous
  commit-time consumer with no re-scan of the shared
  domain.
- 1 integration test
  (`incremental_view_refresh.rs`) covers four consecutive
  mixed commits and asserts incremental-vs-rescan parity
  after each.
- `CommitReceipt` grows by one field; no format
  change, no breaking API change for the 0293 consumer
  (the upserts map is still populated identically).
- Unblocks ADR 0295 (HTAP freshness semantics), which
  can now annotate "view is up-to-date to LSN X" using
  the receipt's `lsn`.
