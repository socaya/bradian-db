# In-memory MvccStore version admission

This Master Gate 1 / G1-MEMORY policy bounds the in-memory version
history retained by the **opt-in G20** `MvccStore`. The store keeps
historical versions in memory for active snapshots; the chain prunes
after commits and after reader exits, but a long-lived reader combined
with steady writes can retain arbitrarily many versions before pruning
catches up. These caps are not durable budgets — the disk state is
bounded separately by `RowStore` and checkpoint policy.

| Budget | Default | Accounted state |
|---|---|---|
| Distinct keys | 1_000_000 | Every key with at least one retained version in `versions: BTreeMap<Vec<u8>, Vec<Version>>` |
| Total versions | 4_000_000 | Sum of chain lengths across all distinct keys |

The caps are **per-store**, not global. `ResourceError::MvccLimit {
resource, limit }` is distinct from storage, corruption and other
admission errors.

## Native configuration and accounting

```rust
use bradian_relational::mvcc::MvccStore;
use bradian_relational::runtime_limits::MvccStoreLimits;

let policy = MvccStoreLimits {
    max_distinct_keys: 10_000,
    max_total_versions: 100_000,
};
let store = MvccStore::open_with_limits("./mvcc", policy)?;
assert_eq!(store.resource_limits().max_distinct_keys, 10_000);
let stats = store.stats(); // committed_lsn, active_snapshots, pinned_positions, keys_retained, versions_retained
```

`MvccStore::open` keeps its prior signature and uses the default policy.
`resource_limits()` reports the configured policy.

## Refusal and recovery

`open_with_limits` admits each seed version as it is inserted from the
authoritative durable row set. A refusal returns
`ResourceError::MvccLimit` and the WAL is byte-identical to pre-open.

`commit()` admits the **projected post-commit** `(distinct_keys,
total_versions)` **before** any WAL append:
- `new_distinct = pending keys not already in versions`
- `projected_keys = versions.len() + new_distinct`
- `projected_versions = current_total + pending.len()`

A refusal leaves the history, WAL and reader pins exactly as they were.
The staged transaction drops normally on unwind and releases its
snapshot pin through the ordinary `Drop` path. Replacing an existing
key does not charge additional distinct-key capacity.

## Example tightening

For a workload with few keys but long reader chains:

```rust
let analytical = MvccStoreLimits {
    max_distinct_keys: 1_000,
    max_total_versions: 1_000_000,
};
```

For a write-heavy workload with short snapshots:

```rust
let transactional = MvccStoreLimits {
    max_distinct_keys: 1_000_000,
    max_total_versions: 10_000_000,
};
```

Long-lived readers pin the oldest version on their chain, which prevents
`prune()` from reclaiming it. A reader abandoned mid-transaction will
hold capacity until its `MvccTransaction` is dropped. Operators who
observe `stats().versions_retained` climbing toward the cap should
investigate whether a session has been forgotten rather than raising
the cap.

## Scope boundary

`MvccStore` is **opt-in**: the shipped SQL engines (`SqlEngine`,
`DomainSqlEngine`, `HistorySqlEngine`) do not route through it by
default. The admission adds no cost to the shipped default path. The
higher-level `MvccSqlEngine` and `TypedMvccStore` compose `MvccStore` and
inherit its budget automatically.

## Remaining scope

- Per-reader retention quota — a single long-lived reader can still
  pin many versions.
- Cross-store aggregate limits combining `MvccStore × N` with other
  engines.
- Row-value byte budgets inside a version — the single WAL batch is
  still bounded by `RowStore`'s inline-record limit.
- A global process-wide RSS ceiling — §13 owner-decision item in the
  [comprehensive implementation plan](../implementation/BRADIAN_COMPREHENSIVE_IMPLEMENTATION_PLAN.md).

See [ADR 0058](../adr/0058-mvcc-store-admission.md) and
[executed verification](../implementation/BRADIAN_GATE1_MVCC_LIMITS_VERIFICATION_2026_11_14.md).
