# Single-table SQL snapshot, scan and projection limits

This is the next portion of fixed Master Gate 1 task G1-MEMORY. It does not advance
the roadmap or provide a total process-memory guarantee.

The earlier SqlEngine and scoped SqlSession now use ScanControl by default. Explicit
native policy is available through execute_with_control and with_sql_session_control.
Native TableStore callers can use scan_with_control and begin_transaction_with_control;
TableTransaction also exposes scan_with_control. Original administrative/native
methods retain their unlimited compatibility behavior. SQL paths do not fall back
to those methods when a controlled operation fails.

## Accounting stages

| Stage | Enforced budget | Important behavior |
|---|---|---|
| Raw scan/snapshot copy | Rows, encoded bytes, examined keys | Captures the LSN and owned raw rows under one lock; checks before copying |
| Transaction snapshot decode | Decoded rows/bytes, retained key storage | BEGIN fails if the complete snapshot exceeds policy; it cannot silently truncate |
| Transaction SELECT | Visible rows/bytes and merged logical-key work | Merges borrowed snapshot/staged maps; does not clone the entire snapshot first |
| Result projection, both engines | Returned rows/bytes and projected-cell work | Charges duplicate columns before cloning values; applies to point queries too |

Defaults remain 100000 rows, 16 MiB per accounted buffer stage and 1000000 work units
per relevant stage. Ordered historical queries retain their smaller input ceiling.
Encoded, decoded and projected limits are separate; they are not a combined RSS
reservation. Allocator/container slack, resident store/catalog/version maps, store
open/replay, formatted JSON output and alternate embedded engine paths remain outside
this accounting. One decoded row is transient workspace before its typed-buffer
charge. Work counters count keys or projected cells, not elapsed time or all CPU.

Legacy raw work includes the examined table metadata key, though that one schema
record is excluded from copied-row/byte counts. Every other namespace remains in
the raw image so typed validation still rejects corrupt keys/table IDs; limits do
not hide malformed data by filtering it out.

## Transaction behavior

The earlier transaction engine owns a full initial table image. A small budget can
therefore reject BEGIN even when a later statement would be a point lookup. Raise
policy explicitly for an appropriately sized workload or use an explicitly migrated
historical engine; the command never changes engines automatically.

An error during BEGIN leaves no transaction. A later resource/cancellation error
aborts all remaining staged work under the earlier engine's original full-abort
contract. Its snapshot LSN and conservative conflict checks are unchanged. Native
API callers own rollback/retry decisions; failed controlled reads return no partial
result and do not commit their staged changes. Lazy table metadata initialization
on open remains existing behavior and is outside transaction/query control.

Cancellation is cooperative at SELECT, copy/decode/projection and snapshot boundaries.
It is not an interrupt of DDL, commit, mutex waits or filesystem I/O. A cancelled
control is one-way; new work needs a fresh control/session.

## CLI

The existing flags now apply to every SQL mode, including default autocommit,
--table and --history:

```sh
bradian-db sql --root ./private-data --max-scan-rows 5000 --max-scan-bytes 8388608 --max-scan-work 100000 --execute "SELECT id FROM demo.public.items"
bradian-db sql --root ./private-data --table demo.public.items --max-scan-rows 5000 --execute "BEGIN; SELECT id FROM demo.public.items; COMMIT"
```

No ordering, savepoints, historical syntax or general expressions are added to the
earlier SQL grammar. Historical behavior is described in [the history scan policy](HISTORY_QUERY_LIMITS.md).
See [executed verification](../implementation/BRADIAN_GATE1_SINGLE_TABLE_LIMITS_VERIFICATION_2026_11_13.md).
