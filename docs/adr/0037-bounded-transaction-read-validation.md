# ADR 0037 — Bounded transaction read validation

**Status:** Accepted for opt-in native durable/history SQL transactions.
**Requirement:** BRD-TXN-002, anomaly-prevention evidence for the current local subset.

G42 adds explicit `begin_with_read_validation` to DurableMvccStore and
DurableTypedStore, and `BEGIN [TRANSACTION] WITH READ VALIDATION` to historical SQL.
Ordinary BEGIN and native begin preserve existing snapshot-isolation semantics.
This extension is not an advertisement of the master's full Serializable isolation
level: Read Committed, wider SQL/constraint coverage and production qualification
remain separate gates.

A validating transaction records every point-read key, including absent keys and
reads made by insert/replace/delete. At commit, under the same state mutex used for
WAL publication, any observed key with a committed version newer than the transaction
snapshot causes a read conflict. Both values and tombstones count, including a value
changed and later restored. Validation applies to read-only commits as well. No
changes are published on conflict, and a conflict consumes the transaction and pin.

At most 4096 distinct keys are retained. The next distinct key switches monotonically
to whole-domain validation and clears the key set. Full scans already require the
same conservative domain check. A changed domain LSN then causes a scan conflict.
This bounds observation memory without returning untracked reads or weakening safety.

Savepoint rollback/release never discards observed keys or the whole-domain flag.
Those reads may have influenced the caller, even if the corresponding writes are
reverted. The original snapshot pin remains held until commit, rollback or drop.
Validation and publication are one critical section, so two concurrent transactions
that read each other's keys and write disjoint keys cannot both commit after those
reads. Unrelated point-read/write sets can still commit independently. All transactions
participating in a shared invariant must opt in: ordinary snapshot transactions
continue to permit write skew and are not retroactively strengthened.

SQL AS OF row reads bypass the transaction's snapshot/read tracker, so validating
sessions reject SELECT AS OF and historical DESCRIBE. Clients use a separate ordinary
snapshot session for explicit historical inspection. There is no silent downgrade.
Existing failed-statement/savepoint recovery semantics apply to these errors.

No persistent marker or format change is needed: observations and mode belong to
the scoped transaction only. Restart recovers committed WAL batches as before.
Older SQL engines, analytical storage and distributed execution are unchanged.
