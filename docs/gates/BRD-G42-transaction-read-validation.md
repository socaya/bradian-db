# BRD-G42 — Bounded transaction read validation

**Design:** [ADR 0037](../adr/0037-bounded-transaction-read-validation.md)
**Requirement:** BRD-TXN-002, current single-process native/history SQL subset.

DurableMvccStore and DurableTypedStore expose `begin_with_read_validation`.
Historical SQL exposes the same opt-in mode:

```sql
BEGIN TRANSACTION WITH READ VALIDATION;
SELECT balance FROM bank.public.audit WHERE id=9;
UPDATE bank.public.accounts SET balance=110 WHERE id=1;
COMMIT;
```

Each `execute` still accepts one statement. At COMMIT, every observed point-read key
must have no version newer than the original snapshot. Missing keys and tombstones
are tracked. Validation and the atomic WAL commit share one mutation mutex, so no
other commit can slip between validation and publication. A conflict publishes
nothing and consumes the transaction; callers must retry the whole transaction from
BEGIN, including its reads and application decisions.

All transactions participating in a shared invariant must opt in. Ordinary BEGIN
and native begin preserve earlier snapshot semantics, which can admit write skew.
The `read_validation_enabled` accessors expose the active choice. Savepoint recovery
cannot clear observed keys or change the mode. Read-only commits also validate and
write no WAL batch; their successful receipt remains their snapshot LSN.

Tracking retains at most 4096 distinct point keys. A further distinct read switches
to conservative whole-domain validation. Full scans likewise reject any changed
domain LSN, including inserts into a previously empty result. Unrelated point-only
transactions may still commit independently. All observations survive savepoint
rollback and release, because they may have informed the application.

SELECT AS OF and historical DESCRIBE are rejected inside validating transactions:
those paths read outside the tracked transaction snapshot. Use an ordinary separate
snapshot session for historical inspection. Unsupported isolation syntax is rejected
rather than silently downgraded. Other SQL engines keep their earlier contracts.

This increment provides anomaly-prevention evidence for a bounded local execution
path. It does not advertise the master's full Serializable isolation level, implement
Read Committed, add predicate-range locks or strengthen distributed/analytical
execution. General SQL, secondary/foreign-key enforcement and mixed-workload
qualification remain open. No persistent format changes are introduced.

Thirteen tests cover synchronized write-skew competitors, ordinary-mode compatibility,
all twenty read/write/commit interleavings of two transactions, independent commits,
read-only and missing-key conflicts, ABA/tombstones, empty scans/phantoms, bounded
tracking fallback, savepoint error recovery, original snapshot pins, historical-read
refusal, defaulted inserts and checkpoint/reopen. See the
[executed report](../implementation/BRADIAN_G42_VERIFICATION_2026_11_13.md).
