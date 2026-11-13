# Historical SQL scan limits and cancellation

This is the query-buffer/work portion of Master Gate 1 task G1-MEMORY. It does not
accept Gate 1 or provide a total process-memory limit.

Historical SQL now scans only the requested table's logical BRPK key prefix. It
merges a transaction's own inserts, replacements and tombstones without cloning the
whole domain or staged map. AS OF uses retained versions at the requested LSN.

## Default query policy

| Budget | Default | What is charged |
|---|---|---|
| Visible input rows | 100000 | Every copied visible row, before projection/windowing |
| Encoded buffer bytes | 16777216 | Logical key, payload and pair-vector header, before cloning |
| Decoded buffer bytes | 16777216, separately | Row/vector/value storage and owned text capacity while decoding/projecting |
| Raw scan work | 1000000 | Merged matching-prefix keys and inspected history versions |

ORDER BY/LIMIT/OFFSET queries lower the input-row ceiling to 10000 even if a higher
scan budget is configured. LIMIT never cuts input before sorting. LIMIT 0 still
reads/validates input and can fail a budget or preserve a scan conflict. Point inputs
are not scans, but result projection is now separately charged for returned rows,
bytes and projected-cell work, including duplicate columns and point lookups.
Point SELECT also checks cancellation at query boundaries.

Byte accounting bounds encoded and decoded buffers separately, not their combined
RSS or allocator/container slack. One inline row is decoded before its result-buffer charge;
resident stores, schemas, version maps, JSON output and existing administrative or
legacy transaction snapshots are outside these query-buffer limits. Raw work counts
keys/versions, not mutex wait time, all schema operations or filesystem I/O.

## Native CLI

```sh
bradian-db sql --root ./private-history --history --max-scan-rows 5000 --max-scan-bytes 8388608 --max-scan-work 100000 --execute "BEGIN; SELECT id FROM demo.public.items ORDER BY id LIMIT 20; COMMIT"
```

The root must already be explicitly migrated and private. These flags now also apply to default and --table modes; see the
[single-table policy](SINGLE_TABLE_QUERY_LIMITS.md) for their snapshot semantics. Defaults remain active when flags are absent. Limits reset for each query;
they are not a cumulative session or server reservation. Configured zero budgets
are valid and cause a scan to fail when it needs the corresponding resource.

## Embedding and cancellation

`ScanLimits` and `ScanControl` are in `bradian_relational::runtime_limits`.
`HistorySqlEngine::with_session_control` accepts an explicit control. Clone the
handle before passing it to the session; another thread can call `cancel()`.
Native durable and typed APIs also expose explicit prefix/scan-with-control methods.

The cancellation flag is shared and one-way. Scans check it for each key/version,
during decoded row accounting, and before/after bounded sorting. Cancellation is not
an interrupt inside a mutex wait or an I/O operation. ROLLBACK remains available
while cancelled. Start a fresh session/control for further queries; ROLLBACK TO does
not reset a cancellation flag.

## Failure and compatibility

Resource errors identify rows, bytes or work, or report query cancelled. They are
not corruption errors. No partial scan result is returned and no resource marker is
written to the WAL. Normal statement failures abort staged work; with a savepoint,
explicit rollback recovery is required. Scan conflict observations survive failed
scans and savepoint rollback. Snapshot pins release on normal abort, full rollback,
conflict or scope drop.

Existing unlimited native/administrative scan APIs keep their compatibility behavior
so query-policy changes do not silently cap migration/backup verification. Historical
SQL calls controlled methods directly and has no unbounded fallback. [Retained-history limits](RETAINED_HISTORY_LIMITS.md) now bound per-domain growth and derived-history reopen admission, with bounded pruning batches. [Backing-map limits](ROW_STORE_LIMITS.md) also control RowStore checkpoint/replay maps. Other embedded engines, metadata/index/checkpoint/backup working memory and global resident-memory enforcement remain separate Gate 1 work.
