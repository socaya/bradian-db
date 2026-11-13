# Backing RowStore map limits

This fixed Master Gate 1 / G1-MEMORY policy acts before derived history admission:
it controls the live-row map and committed WAL-delta map while opening and writing
a native RowStore. Existing open consumers use defaults, including SQL/history
backing domains. Each RowStore is charged separately.

| Budget | Default | Accounted state |
|---|---|---|
| Live rows | 2000000 | Every live physical key, including metadata |
| Live-row bytes | 268435456 | Key/value lengths and native pair-vector headers |
| Delta keys | 2000000 | Every key changed since checkpoint, including deleted/absent keys |
| Delta bytes | 268435456 | Delta key/value lengths and native vector/option headers |

These independent byte ceilings do not include allocator/BTree node slack or provide
a combined RSS reservation. A key changed repeatedly occupies one final delta entry.
Deleting a key refunds its live-row bytes but retains a delta tombstone. All physical
history versions count as separate RowStore keys, in addition to the history layer's
separate logical/version policy. Format/floor metadata also counts here.

## Native configuration and accounting

```rust
use bradian_relational::row_store::RowStore;
use bradian_relational::runtime_limits::RowStoreLimits;

let policy = RowStoreLimits {
    max_rows: 100000,
    max_row_bytes: 64 * 1024 * 1024,
    max_delta_keys: 100000,
    max_delta_bytes: 64 * 1024 * 1024,
};
let db = RowStore::open_with_limits("./private-store", policy)?;
let usage = db.resource_usage();
```

resource_usage reports rows, row_bytes, delta_keys and delta_bytes; resource_limits
reports the selected immutable policy. Existing RowStore::open uses defaults. Limits
are not persisted or a format migration. Higher-level SQL/history APIs use default
backing-map policy; their existing query, staging and retained-history flags do not
change these ceilings. Custom backing-map policy is currently a native RowStore API.

## Refusal and recovery

Loading a checkpoint checks row/key bytes before copying each entry into the live
map. WAL replay validates a bounded complete frame before admitting its final changes.
Every replayed committed prefix must fit. A small final state does not let a lower
policy skip an earlier large checkpoint or replay state. Raise policy, open and
explicitly checkpoint the smaller state before retrying a lower policy where suitable.

Put/delete and batch commit check final live/delta usage under the mutation mutex
before encoding/appending WAL. Repeated keys coalesce to their final operation for
admission and application; WAL serialization itself keeps the original operations.
Affected old entries are dropped before copying final entries, avoiding transient
map growth during a key swap. Refusal changes no WAL bytes, committed rows, LSN or
usage and does not poison the healthy owner. Conflicts and corruption still refuse
under their existing contracts.

Open quota refusal returns without trimming an incomplete WAL tail. Successful open
can still perform normal incomplete-tail repair and missing-index reconstruction.
A quota error is not proof that all later persisted bytes have passed integrity checks.
The open path creates/locks its normal directory/lock/WAL files as applicable; it is
not a read-only audit API.

A successful checkpoint publishes the new generation and synchronously truncates the
WAL before clearing/refunding delta state. At a delta ceiling, checkpointing can free
capacity without deleting live rows. No checkpoint, history pruning or data discard
runs automatically on resource refusal. Failed I/O retains existing poisoning/reopen
rules; no failed-I/O qualification is added by this policy.

Backup verification uses the source policy, including checkpoint-plus-WAL backups.
RowStore::restore_backup_with_limits(archive, destination, policy) validates the
staged destination under its explicit policy. The existing restore entry points use
defaults. Refusal never publishes the destination or changes archive/source rows;
the incomplete staging directory follows the existing cleanup/fresh-destination
contract. Backup comparison visits borrowed recovered rows, avoiding another whole
map copy. Archive format and receipts are unchanged.

[Index recovery controls](INDEX_RECOVERY_LIMITS.md) now separately admit retained key/pointer entries and remove recovery value-map/second-leaf-list copies. Native open_with_recovery_limits and restore_backup_with_recovery_limits accept both backing and recovery policies; ordinary backing APIs retain default recovery policy.

## Remaining scope

These are accounted map limits. Frame decoding (up to the existing 1 MiB bound), page
decoding, directory/metadata input, derived-index rebuild/audit, checkpoint construction,
backup verification working buffers and caches are separate. They are not a guarantee
against allocation failure, OOM, malicious-file resource use or total RSS growth.
Global admission across stores, higher-level catalog state, active transactions/pins
and alternate analytical engines still need work. No disk-full, physical power-loss
or cross-platform certification is claimed.

See [ADR 0045](../adr/0045-backing-row-map-admission.md),
[retained-history policy](RETAINED_HISTORY_LIMITS.md) and
[executed verification](../implementation/BRADIAN_GATE1_ROW_STORE_LIMITS_VERIFICATION_2026_11_13.md).
