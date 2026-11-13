# ADR 0036 — Bounded historical transaction savepoints

**Status:** Accepted for native durable MVCC and scoped historical SQL sessions.

G41 adds named, in-memory checkpoints of a transaction's staged key mutations.
A transaction holds at most 16 markers; each marker copies at most 256 staged keys
with the existing inline key/value limits. Names are ASCII identifiers of at most
63 bytes. Native names are case-sensitive; SQL folds unquoted identifiers as before.
No marker, rollback or release writes the WAL or changes the pinned snapshot LSN.

SAVEPOINT pushes a marker; duplicate names shadow earlier markers. ROLLBACK TO
restores the most recent matching staged map and discards later markers while
retaining the target marker. RELEASE discards the matching marker and later ones,
keeping staged writes. Neither operation commits. Only the outer commit writes one
atomic WAL batch. Drop/full rollback discards all markers and releases the pin.

The conservative full-scan flag is monotone, including across rollback-to: readers
must not lose conflict protection when reverting their writes. Per-key conflict
checks apply to the final staged write set under the original snapshot. This does
not provide serializable isolation or predicate locks.

Historical SQL adds SAVEPOINT name, ROLLBACK TO [SAVEPOINT] name and RELEASE
[SAVEPOINT] name inside BEGIN. Without markers any statement error still aborts all
staged work. With markers an error retains the transaction in a failed state;
only successful ROLLBACK TO or full ROLLBACK can recover it. COMMIT and further
statements cannot publish earlier writes in that state. Commit conflicts consume the
transaction as before. Missing/invalid marker operations leave the staged map intact.

Other SQL engines keep their existing contracts. Markers are not durable sessions,
nested commits or a network protocol; no physical format changes are introduced.
