# BRD-G41 — Bounded durable transaction savepoints

**Design:** [ADR 0036](../adr/0036-bounded-history-transaction-savepoints.md)

The native DurableMvccTransaction and DurableTypedTransaction APIs support named
savepoints, rollback-to and release. Scoped HistorySqlSession exposes the same
operations through SQL inside BEGIN:

```sql
BEGIN;
UPDATE bank.public.accounts SET balance=110 WHERE id=1;
SAVEPOINT optional_work;
UPDATE bank.public.audit SET balance=999 WHERE id=9;
ROLLBACK TO SAVEPOINT optional_work;
RELEASE SAVEPOINT optional_work;
COMMIT;
```

Each `execute` accepts one statement, as before. In this example only the account
update commits; the audit update is discarded. SAVEPOINT/ROLLBACK TO/RELEASE never
write the WAL. The outer COMMIT publishes one atomic batch across registered tables.

The most recent duplicate name shadows earlier markers. ROLLBACK TO retains its
target marker and discards later ones. RELEASE discards its target and later markers
while keeping staged writes. Names are identifiers of at most 63 ASCII bytes;
SQL applies its existing unquoted-name case folding. There are at most 16 markers,
each containing at most 256 staged keys under existing inline key/value bounds.

Statement errors with markers retain a failed transaction. COMMIT and further work
are blocked until successful ROLLBACK TO or full ROLLBACK. Without markers an error
still discards all pending work. Commit conflicts consume the transaction. Scope
exit discards remaining staged work and markers. Snapshot pins and full-scan/per-key
conflict observations are preserved across savepoint operations.

This is historical SQL and native durable MVCC functionality; earlier SQL engines
retain their existing behavior. It does not add nested commits, persistent/network
sessions, serializable isolation or a new persistent format.

Twelve new tests verify cross-table atomic selection of writes, shadowing, release,
failed-state recovery, capacity/name refusal, callback cleanup, pins/retention,
concurrent key and scan conflicts, required column defaults, checkpoint/reopen and
process exit before/after outer commit. See the
[executed report](../implementation/BRADIAN_G41_VERIFICATION_2026_11_13.md).
