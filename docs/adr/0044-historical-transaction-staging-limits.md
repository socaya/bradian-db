# ADR 0044 — Historical transaction staging limits (G1-MEMORY)

**Status:** Accepted for native durable MVCC, typed historical transactions and historical SQL.

The 256-key physical WAL batch ceiling was previously checked only at COMMIT.
Native callers could retain arbitrarily many staged keys before that refusal.
Savepoints separately capped marker/key counts, but copied maps did not share a
byte policy with current writes. Enforce both controls at staging boundaries.

StagingLimits defaults to 256 distinct current keys and 8 MiB of accounted bytes
across the current pending map and all saved map copies. A supplied key policy can
lower the ceiling but cannot raise the physical 256-key maximum. Existing 16-marker
and 63-byte identifier limits remain. The byte estimate charges each map entry's
key/value bytes and native Vec/Option headers, plus each saved marker's native
header, name bytes and copied entries. Container/allocator slack, fixed transaction
headers, read observations/pins, decoded statement inputs, scan/output buffers and
commit serialization are separate. This is not RSS or a global reservation.

Perform checks before copying into the pending map or cloning a savepoint. Replacing
a staged value refunds its old charge; a tombstone still consumes one key/header.
Rejected native writes/markers retain prior staged state and do not append WAL.
Read observations made while validating a refused mutation remain monotone.

ROLLBACK TO computes and checks restored usage, drops current writes and descendants,
then copies the target map. This avoids retaining discarded maps during the restore
copy. The target marker and conflict observations remain. RELEASE refunds discarded
markers/copies while preserving current writes. Duplicate-name lookup continues to
select the latest marker. Drop/rollback/commit retain their existing snapshot cleanup.

Native and typed begin_with_limits accept an immutable per-transaction staging policy
and the explicit read-validation mode. Existing BEGIN APIs use defaults. Historical
SQL with_session_limits accepts query and staging policies independently; every new
BEGIN uses the selected staging policy. SQL resource errors retain existing behavior:
no marker means full abort; with a marker, only explicit rollback recovery can resume.
Native, typed and SQL accessors expose accounted usage. CLI staging flags require
--history; unsupported modes refuse them. No SQL grammar, persistent format,
retention policy, default isolation or dependency change is introduced.

The change intentionally moves the 257th distinct-key refusal from COMMIT to the
mutation itself. Native callers may recover or commit the first 256 valid writes;
SQL applies its existing statement-error rules. WAL byte-size validation and retained
history admission still occur independently at COMMIT. Aggregate session/pin limits,
other engines' staging, backing-store open/replay and process-wide admission remain
open Gate 1 work.
