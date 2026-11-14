# Master Gate 1 — In-memory MvccStore admission verification

14 November 2026. Baseline: `55db0cb`.
Stable task G1-MEMORY in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**845 Rust tests passed, 0 failed, 0 ignored**. Clippy clean on
`bradian-relational --all-targets`. CI remains manual-only. No persistent
format, wire protocol or SQL grammar change.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace` | 845 passed, 0 failed |
| `cargo test -p bradian-relational --test mvcc_limits` | 5 passed |
| `cargo test -p bradian-relational --test mvcc` | 14 passed (unchanged) |
| `cargo clippy -p bradian-relational --all-targets -- -D warnings` | Passed |

## Implemented and exercised

`MvccStoreLimits { max_distinct_keys, max_total_versions }` is added to
`runtime_limits.rs` with defaults 1_000_000 and 4_000_000.
`ResourceError::MvccLimit { resource, limit }` is distinct from storage
and other admission errors.

`MvccStore::open_with_limits(root, limits)` accepts an explicit policy;
`open` retains its signature and uses the default.
`resource_limits()` exposes the configured policy. Admission runs in
two places:

1. `open()` — after each version is seeded from durable rows, projected
   `(distinct_keys, total_versions)` is admitted. A refusal returns
   `ResourceError::MvccLimit` and the WAL is byte-identical to pre-open.
2. `commit()` — admission runs on projected post-commit
   `(distinct_keys, total_versions)` **before** any WAL append. Refusal
   leaves the history, WAL and reader pins exactly as they were; the
   staged transaction is dropped through the ordinary `Drop` path.

Five new tests cover (1) live refusal of a third distinct key with WAL
preservation, (2) live refusal of a third version while a pinned reader
holds the oldest version, (3) replacing an existing key not charging
additional distinct capacity, (4) tighter reopen refusal without
mutating the WAL, (5) default policy admitting an ordinary 64-key
workload. All 14 existing MVCC tests remain green under the amended
store.

## Remaining scope

- Per-reader retention quota.
- Cross-store aggregate limits (`MvccStore × N` alongside other engines).
- Global process-wide RSS ceiling — owner-decision item in the
  comprehensive implementation plan §13.
- Row-value byte budgets inside a version.

See [ADR 0058](../adr/0058-mvcc-store-admission.md) and the
[fixed roadmap](BRADIAN_MASTER_ROADMAP.md).
