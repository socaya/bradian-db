# Checkpoint and index build limits

This fixed Master Gate 1 / G1-MEMORY policy admits a checkpoint's prospective owned
key/pointer list and an immutable index builder's conservative descriptor allowance.
Index descriptors now borrow separator keys rather than cloning them at every level.

| Budget | Default | Accounting |
|---|---|---|
| Input entries | 2000000 | One key/pointer entry per live checkpoint row |
| Conservative build bytes | 536870912 | Key lengths, native key/pointer tuple headers and two borrowed descriptor headers per input key |

A descriptor contains a u64 page ID and a borrowed byte slice. Child and parent
levels coexist during promotion, so two descriptors per input key safely cover their
logical count. The reservation usually exceeds actual logical descriptor usage.
Allocator/vector slack and fixed page/I/O buffers are excluded. Source live/delta
maps, recovery-entry storage and caches retain independent policies; this is not a
global memory pool or total RSS limit.

## Configure native policies

```rust
use bradian_relational::row_store::RowStore;
use bradian_relational::runtime_limits::{StorageLimits, IndexBuildLimits};

let policies = StorageLimits {
    build: IndexBuildLimits { max_entries: 100000, max_bytes: 64 * 1024 * 1024 },
    ..StorageLimits::default()
};
let db = RowStore::open_with_policies("./private-store", policies)?;
let lsn = db.checkpoint()?;
```

StorageLimits has rows (RowStoreLimits), index (IndexLimits) and build
(IndexBuildLimits). storage_limits returns the owner policies. Existing open,
open_with_limits and open_with_recovery_limits keep their signatures and use default
build policy. Higher SQL/history APIs and CLI modes use native defaults; their query,
staging/history flags do not override build policy. No policy is persisted.

checkpoint_with_limits provides a one-operation override without changing owner
policy. A current checkpoint can return successfully under zero build limits because
it needs no new build. Empty checkpoint/index construction also fits zero limits.
DiskBTree::build_with_limits takes an explicit build policy; the original build uses
defaults and produces identical bytes for the same inputs.

## Refusal and recovery

Checkpoint preflight runs under the same mutation mutex as publication, before
orphan quarantine, temporary-file creation/truncation, snapshot/index publication or
WAL truncation. Refusal preserves rows, delta/usage, LSN, manifest/WAL bytes and prior
temporary/unpublished files. The healthy owner remains writable. Raise policy and
retry explicitly; no automatic data discard or retention change is used.

The builder checks its inputs and conservative reservation before creating a target.
Missing-index recovery preflights build policy after bounded checkpoint-entry loading
and before removing its old rebuild temporary file. A failed automatic rebuild leaves
the index absent and WAL tail untouched. Raising policy permits a valid rebuild and
then ordinary incomplete-tail repair. Existing published indexes are never silently
overwritten. The earlier recovery-entry limits remain independent.

After successful preflight, ordinary I/O/device failures still use existing fencing,
poisoning and recovery rules. A preflight budget is not a guarantee that allocations
or writes will succeed. Checkpoint publication and WAL truncation/sync retain their
existing order, and only a completed checkpoint refunds delta charges.

Backup verification carries all source policies. RowStore::restore_backup_with_policies
accepts explicit destination policies and verifies its stage before publication.
If an archive already contains a valid index, restore audit does not build one and
therefore does not consume a build reservation. The selected process-local policy
must be passed again when reopening; it is never saved in an archive. Existing backup
and incomplete-stage handling remain unchanged.

## Remaining scope

This controls accounted key/pointer/descriptor working state, not all process memory.
Metadata/directory inputs, allocator/container slack, caches, catalogs, active
transaction/pin totals, formatted output, alternate engines and global admission
remain open. No cancellation, allocation-failure, disk-full, failed sync/rename,
physical power-loss or cross-platform qualification is established by these limits.

See [ADR 0047](../adr/0047-checkpoint-build-admission.md),
[index recovery limits](INDEX_RECOVERY_LIMITS.md) and
[executed verification](../implementation/BRADIAN_GATE1_CHECKPOINT_LIMITS_VERIFICATION_2026_11_13.md).
