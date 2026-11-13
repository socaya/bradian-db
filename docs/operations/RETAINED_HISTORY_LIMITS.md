# Retained-history admission limits

This is part of fixed Master Gate 1 task G1-MEMORY. Each durable history domain now
has an explicit policy for retained state, separate from per-query scan limits.
Existing open APIs use defaults; policy is not persisted in the database or archive.

| Budget | Default | Charged state |
|---|---|---|
| Logical keys | 1000000 | Every retained user/metadata key, including tombstone baselines |
| Retained versions | 1000000 | Each committed value or tombstone version |
| Accounted bytes | 268435456 | Logical-key entry, derived version header/payload and backing physical history key/value/header |

The byte estimate uses native Rust header sizes. It excludes fixed format/floor
records, allocator/container slack, RowStore overlays/caches, active transactions,
pins, typed catalog/epoch structures, staging and temporary validation/output
buffers. It is not process RSS or a combined global reservation. Initial RowStore
open/replay precedes the derived-history admission check and now uses separate default
[backing-map limits](ROW_STORE_LIMITS.md). Those limits do not cover all recovery workspace.
Borrowed backing-row traversal avoids an additional complete raw snapshot clone.

## CLI and native policy

```sh
bradian-db sql --root ./private-history --history --max-history-keys 1000000 --max-history-versions 2000000 --max-history-bytes 536870912 --execute "BEGIN; SELECT id FROM demo.public.items; COMMIT"
```

These flags require --history. Zero is valid and refuses any corresponding retained
state; fixed format/floor markers do not consume these budgets. Scan flags still
apply independently in every SQL mode. Raising a limit permits larger valid state;
it does not disable corruption, schema, snapshot or conflict checks.

Native callers use runtime_limits::HistoryLimits with DurableMvccStore::open_with_limits,
DurableTypedStore::open_with_history_limits or HistorySqlEngine::open_with_history_limits.
The history_usage/history_limits accessors report current accounted state and policy.
Policy-aware shared and lifecycle migration and restore entry points end in
_with_limits. Existing entry points use defaults. Backup verification carries the
source policy, while restore accepts its own explicit policy.

The separate [transaction staging policy](HISTORY_STAGING_LIMITS.md) now controls current historical writes and saved copies. It does not charge them to the retained-state budget.

## Refusal and recovery

Commit checks the complete final staged key set under the mutation mutex before
encoding/appending the WAL batch. A quota failure publishes no part of that batch,
no new LSN and no counter changes. Replacements and deletes consume new versions;
a delete is not an immediate history refund. Savepoint-discarded work is uncharged.
Concurrent writers cannot both consume the same remaining capacity.

Open reconstructs exact counters and refuses over-policy history instead of silently
pruning it. Normal incomplete-WAL-tail repair retains its existing contract. To
reduce a policy below current usage, first open with sufficient limits, explicitly
prune to an acceptable floor, then reopen under the lower limits. No automatic
retention policy chooses which historical reads an operator must lose.

Historical DDL checks both intent and activation capacity before writing its intent.
Interrupted activation checks remaining capacity before catalog repair. Raise policy
to finish a valid interrupted operation; capacity refusal must not be treated as
corruption. Existing I/O failure fencing remains in place.

Explicit prune_through preserves pinned snapshots and writes the durable floor
before deleting obsolete versions. The deletion worklist is at most 256 entries;
counters are refunded only after each synced deletion batch. Per-key value/tombstone
baselines and logical-key charges remain. Unused version-vector capacity is requested
back after pruning, without an allocator/RSS reclamation guarantee. Reopen after a
committed deletion prefix rebuilds its exact usage, and pruning can resume safely.

A migration/restore that exceeds policy never publishes its destination or changes
source rows. Its incomplete staging directory follows the existing refusal contract;
use a fresh destination or explicit operator cleanup before retrying. No archive,
WAL, page or schema format changes are required.

Memory pressure, allocation failure, device failures and full Gate 1 acceptance are
not established by these accounted limits. See [ADR 0043](../adr/0043-retained-history-admission.md)
and [executed verification](../implementation/BRADIAN_GATE1_HISTORY_ADMISSION_VERIFICATION_2026_11_13.md).
