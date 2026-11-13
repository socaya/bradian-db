# Bradian G42 executed verification — 13 November 2026

## Result

**636 passed, 0 failed, 0 ignored**, including **381 relational tests**, across
all Rust workspace features. Workspace compilation, strict Clippy across all targets
and features, and Studio typecheck/build passed. Baseline:
`581f82ca5cdfe9dcb6d55b2908e1e211c5ad88ab`.

Environment: Linux x86_64, Rust/Cargo 1.100.0 stable, Node 24.22.1, npm 11.9.0.
No dependencies, lockfiles or persistent formats changed. Automatic CI remains
manual-only. Existing unused manifest dependency/field advisories remain.

| Executed command | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 636 passed; no failures or ignored tests |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| `npm run typecheck` in `apps/dap-ui` | Passed |
| `npm run build` in `apps/dap-ui` | Passed |

## Delivered and verified

G42 adds explicit read-validation transactions to the native durable store, typed
adapter and historical SQL. Observed point keys, including missing values and
tombstones, are checked atomically with commit publication. Read-only commits also
validate. Tracking falls back to whole-domain validation after 4096 distinct keys;
scans use the existing conservative domain check. Savepoints cannot erase reads.

Seven native tests verify synchronized competing writers, rejection of write skew
when both participants opt in, unchanged ordinary snapshot behavior, independent
validated writers, read-only commits without WAL publication, missing keys,
tombstones and ABA values, savepoint-preserved observations, empty scans/phantoms,
capacity fallback and pin cleanup. A schedule test enumerates all twenty
interleavings of two transactions' read/write/commit steps, checks the shared rule
and requires at least one successful commit in each schedule.

Six SQL integrations verify cross-table decisions, default-mode compatibility,
read-only and absent-row conflicts, full rollback on commit conflict, savepoint
error recovery, original snapshot preservation, empty-scan phantoms, strict mode
syntax, refusal of historical reads that bypass validation, independent transactions,
required column defaults, callback cleanup and checkpoint/reopen. The full earlier
analytical, relational, HTTP, migration, backup, corruption and recovery suites pass.

## Limits

This is opt-in read validation for the bounded local native/history SQL path.
Every transaction participating in a shared invariant must opt in; ordinary BEGIN
can still permit write skew. This does not advertise the master's full Serializable
isolation level, implement Read Committed, add predicate-range locking or strengthen
other SQL engines/distributed/analytical execution. Historical reads require a
separate ordinary snapshot session. General SQL, constraint enforcement, production
workload qualification and enterprise security remain open.

No browser interaction, physical power-loss, disk-full or cross-platform recovery
qualification was run. See the [gap register](BRADIAN_IMPLEMENTATION_GAPS.md).
