# ADR 0042 — Bounded single-table SQL scans and snapshots (G1-MEMORY)

**Status:** Accepted for the earlier single-table SQL/CLI path; Gate 1 stays active.

Extend the existing ScanLimits/ScanControl policy to SqlEngine autocommit SELECT
and scoped SqlSession. Their defaults match historical SQL. A RowStore controlled
snapshot copies rows and captures its base LSN under one mutation lock, charges raw
row/byte/work budgets before copying and checks cancellation cooperatively. The
single known table-schema metadata key is excluded from output budgets, while every
examined key consumes work. Other namespaces are not filtered away: the typed layer
still rejects unexpected keys or mismatched row payloads.

TableStore controlled scan and transaction initialization separately charge decoded
buffers, including owned text and retained snapshot keys. The earlier transaction
model requires an owned initial snapshot; BEGIN therefore fails if that snapshot
exceeds policy, even when the intended next statement is a point lookup. It must not
truncate input or change snapshots between statements to work around a limit.
Transaction SELECT merges borrowed snapshot/staged maps and clones only budgeted
visible results, avoiding the previous full-map clone. Snapshot LSN, first-conflict
and statement-error/full-abort semantics stay unchanged. No savepoints or historical
syntax are added to the earlier engine.

SqlEngine execute_with_control and with_sql_session_control expose explicit native
policy; ordinary SQL methods now use default controls. Existing TableStore/native
transaction scan/snapshot APIs remain unlimited compatibility paths for administrative
callers. No SQL query falls back to them. Cancellation checks apply to SELECT and
snapshot initialization, not as an interrupt of DDL, commit, filesystem I/O or waits.
A failure returns no partial result and does not publish staged mutations. Existing
lazy table-metadata initialization remains part of open, not a transactional query.

CLI scan flags now apply to default, --table and --history modes. Removing their
--history-only restriction is deliberate because every advertised mode now enforces
them on its scan/snapshot path. Flags are not silently ignored. Point reads are not
full scans, except that the legacy transaction's initial snapshot still needs a budget.

Result projection in both SQL engines additionally charges returned rows, requested
value/string copies and one work unit per projected cell before cloning. This closes
repeated-column amplification, including point queries: result row/byte/work policy
now applies even when the input is not a scan.

These are accounted per-stage buffer/work limits, not total RSS/admission limits.
Resident RowStore/catalog/history maps, output serialization, allocator/container
slack and alternate embedded engines remain separate Gate 1 work. Store-open and
replay allocations are unchanged. No persistent format, grammar, third-party version
or migration behavior changes are introduced.
