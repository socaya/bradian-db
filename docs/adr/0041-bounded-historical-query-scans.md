# ADR 0041 — Bounded historical query scans (G1-MEMORY)

**Status:** Accepted for Gate 1 query-buffer/work limits; not complete runtime qualification.

Historical typed scans currently clone the entire durable MVCC domain and filter
by table afterwards. Replace that path with a prefix range over the requested table's
BRPK/version/table-ID namespace. Transaction scans merge committed and staged keys
in order without first cloning either map; staged values/tombstones take precedence.
Historical scans choose the visible retained version under the same mutation lock.
Snapshot and whole-domain scan-conflict observations remain unchanged.

Public ScanLimits/ScanControl define per-operation row, owned-byte and work limits
plus a shared cancellation flag. Defaults are 100000 rows, 16 MiB and 1000000 work
steps. Every merged logical key and inspected committed version consumes work. Encoded keys/payloads
are charged before cloning. Typed result buffers separately charge row/vector/value
and owned text capacity while decoding/projecting. No partial result escapes a failed
scan. Resource-limit/cancellation errors are distinct from corruption and conflicts.

Raw encoded and decoded buffers are each bounded, not a single RSS reservation.
Allocator/container slack, schema parsing, resident store/catalog/version maps and existing
legacy native scan APIs are outside this buffer accounting. This is not streaming from
disk, eviction, a total process-memory cap, cancellation of mutex waits or interrupts
inside filesystem I/O. The older SQL engines are not silently switched to history.

Historical SQL uses default limits for scans; embedding callers may supply a control
to with_session_control and inspect/cancel it from another thread. Ordered/windowed
queries lower their input-row budget to the existing 10000-row ceiling before the
scan. LIMIT 0 still scans/validates and cannot erase conflict observations. Cancellation
is checked during raw scans, typed decoding and before/after bounded sorting. Point
query cancellation is checked at query boundaries. Full ROLLBACK remains usable after
cancellation. G41 error/savepoint behavior applies; cancelling does not commit work.

The native CLI exposes --max-scan-rows/bytes/work only with --history; other modes
reject those options instead of ignoring them. Budgets are in-memory policy, require
no persistent format changes, and do not weaken retained-history validation, current
schema binding, original snapshots or backup/restore verification. Whole-store open,
retention/checkpoint/backup and legacy transaction snapshots remain separate work.
Existing administrative typed scan methods retain their unlimited compatibility
contract; SQL uses controlled methods directly and never falls back to those paths.
Cancellation flags are one-way for the control handle: rollback releases work/pins
but does not reset cancellation; start a new session/control to run further queries.
