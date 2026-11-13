# Derived-index recovery limits

Fixed Master Gate 1 / G1-MEMORY now bounds the checkpoint key/pointer entries retained
by an explicit index audit or missing-index rebuild. Recovery no longer clones all
checkpoint values, and leaf comparison no longer builds a second complete index list.

| Budget | Default | Charged state |
|---|---|---|
| Retained entries | 2000000 | One key/pointer tuple per checkpoint row |
| Retained bytes | 268435456 | Key length plus native Vec/RowPointer tuple header |

Entry/byte checks happen before each key copy. Physical pages and record lengths,
row order, count and CRCs are still validated, including value bounds. Values remain
in the decoded page and do not consume retained-entry bytes. One previous key of at
most 512 bytes, allocator/vector slack, page/cache storage and builder descriptors
are outside the estimate. This is not RSS or a complete checkpoint memory budget.

## Native APIs

runtime_limits::IndexLimits has max_entries and max_bytes. Existing ordinary APIs use
default policy unless a RowStore owner was opened with explicit recovery limits.

```rust
use bradian_relational::row_store::RowStore;
use bradian_relational::runtime_limits::{RowStoreLimits, IndexLimits};

let recovery = IndexLimits { max_entries: 100000, max_bytes: 16 * 1024 * 1024 };
let db = RowStore::open_with_recovery_limits("./private-store", RowStoreLimits::default(), recovery)?;
let report = db.verify_checkpoint_index()?;
```

verify_checkpoint_index_with_limits and rebuild_missing_checkpoint_index_with_limits
accept an operation-specific override. The standalone index_recovery::verify_with_limits
API also accepts explicit policy, with the same caller-owned offline/exclusive-access
contract as the original standalone verifier. RowStore methods hold the store mutex.
Policies are process-local and never persisted in snapshot/WAL/index/archive bytes.

open_with_recovery_limits uses its policy for automatic missing-index reconstruction.
The ordinary open/open_with_limits entry points choose default recovery policy.
Existing-index opens retain their normal manifest checks; a full audit is explicit.
Higher SQL/history APIs and the CLI continue to use default index recovery policy;
query/history/staging flags do not configure it.

[Checkpoint/build admission](CHECKPOINT_BUILD_LIMITS.md) now independently covers pointer construction and a conservative borrowed-descriptor allowance. StorageLimits composes all three native storage policies; open_with_policies and restore_backup_with_policies accept that composition. Earlier entry-only APIs retain default build policy.

## Refusal, verification and backup

A quota error during entry collection precedes temporary-file cleanup and index
creation. It preserves snapshot/WAL bytes and leaves a missing index unpublished.
Open refuses before normal incomplete-WAL-tail repair; raising policy and reopening
can rebuild and then perform the normal tail repair. Existing published indexes are
never overwritten implicitly, including when corruption has a valid page CRC.

The verifier compares the complete ordered leaf sequence incrementally and retains
point-lookup equivalence checks for every expected pointer. Missing/extra entries,
wrong pointers, invalid order and broken leaf chains remain errors. Native range
queries retain their owned-result API and existing behavior. Leaf record counts that
cannot physically fit their page are rejected before entry-vector reservation.

Backup verification inherits the source owner's selected recovery policy.
RowStore::restore_backup_with_recovery_limits(archive, destination, backing, recovery)
verifies a staged destination under both explicit policies before publishing it.
Existing restore APIs use default recovery policy. Failure never publishes the final
destination or alters source/archive rows; incomplete staging retains its existing
cleanup/fresh-destination contract. backup_to may checkpoint before later verification
refuses, as under its existing checkpoint-then-backup contract.

A resource refusal does not certify unvisited persisted bytes as valid. Retained
entries are bounded, but builder descriptors, checkpoint pointer construction,
metadata/directory buffers, caches, global admission and total process memory remain
separate work. No cooperative cancellation, OOM/device-failure or cross-platform
qualification is added by this policy.

See [ADR 0046](../adr/0046-bounded-index-recovery-entries.md) and
[executed verification](../implementation/BRADIAN_GATE1_INDEX_RECOVERY_LIMITS_VERIFICATION_2026_11_13.md).
