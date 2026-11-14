# ADR 0058 — Opt-in `MvccStore` version admission (G1-MEMORY)

**Status:** Accepted for `mvcc::MvccStore` in-memory version history.
Gate 1 remains active. This does not accept the full master G1-MEMORY exit
(global process ceiling, cross-engine totals).

`MvccStore` is the opt-in G20 foundation: a native, WAL-backed row-level
MVCC kernel whose in-memory `versions: BTreeMap<Vec<u8>, Vec<Version>>`
retains historical versions for pinned readers. The chain is pruned after
commits and after reader exits, but a long-lived reader combined with
steady writes can retain arbitrarily many versions before pruning catches
up. No admission check existed for either distinct-key count or total
version count.

`MvccStoreLimits { max_distinct_keys, max_total_versions }` is added to
`runtime_limits.rs` with defaults 1_000_000 and 4_000_000. Defaults admit
an ordinary workload (the existing MVCC suite is well under both caps)
while putting a conservative upper bound on in-memory growth. The limits
are **per-store**, not global: one process can legitimately own several
`MvccStore` instances and each has its own budget.
`ResourceError::MvccLimit { resource, limit }` is distinct from storage,
corruption and other admission errors.

`MvccStore::open_with_limits(root, limits)` accepts an explicit policy
and records it on the store. The default `MvccStore::open` retains its
existing signature and uses `MvccStoreLimits::default()`.
`resource_limits()` exposes the configured policy. Policies are not
persisted.

Enforced invariants:

1. During `open()`, the seed loop admits after each inserted version. A
   refusal returns `ResourceError::MvccLimit` and the WAL is unchanged.
2. During `commit()`, admission runs on the projected post-commit
   `(distinct_keys, total_versions)` **before** any WAL append. A refusal
   leaves the live history, WAL and reader pins exactly as they were;
   the staged transaction is dropped normally on unwind and its snapshot
   pin is released through the ordinary `Drop` path.

Not enforced by this ADR and explicitly open:

- Per-reader retention quota (one reader can still pin many versions).
- Cross-store aggregate limits (`MvccStore` × N alongside other engines).
- A global process-wide RSS ceiling — comprehensive implementation plan
  §13 owner-decision item.
- Row-value byte budgets within a version (the single WAL batch is still
  bounded by `RowStore`'s inline-record limit).

No persistent format, wire protocol, SQL grammar or dependency change.
`MvccStore` is still opt-in — the shipped SQL engines do not route
through it by default, so the admission adds no cost to the shipped
default path.

## Amendment — propagation through TypedMvccStore and MvccSqlEngine

`TypedMvccStore::open_with_limits(root, catalog_tables, mvcc_limits)`
and `MvccSqlEngine::open_with_limits(root, mvcc_limits, catalog_limits)`
accept explicit `MvccStoreLimits` (and `CatalogLimits` for the SQL
engine) and thread them to the underlying `MvccStore` and
`PersistentCatalog`. The default `open` signatures retain the
previous behaviour and use `MvccStoreLimits::default()` /
`CatalogLimits::default()`.

Integration tests in `tests/transitive_admission.rs` prove that a
tight upstream policy refuses via the higher-level entry points
without modifying the on-disk WAL.

