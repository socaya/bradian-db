# Master Gate 1 — Historical transaction staging verification

13 November 2026. Baseline: `d3e4164ada4788a65e29709e580f34fff456cbf6`.
Stable task G1-MEMORY in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**717 Rust tests passed, 0 failed, 0 ignored**, including **447 relational tests**.
Workspace compilation with all features, strict Clippy across all targets/features,
Studio typecheck/build and the fixed-roadmap check passed. Automatic CI stays
manual-only. No dependency, lockfile, persistent format or SQL grammar changed.

Environment: Linux x86_64, Rust/Cargo 1.100.0 stable, Node 24.22.1 and npm 11.9.0.

| Executed command/check | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed; existing manifest warnings remain |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 717 passed |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| Studio `npm run typecheck` and `npm run build` | Passed |
| `python3 scripts/verify_roadmap.py` | 60 unique requirements, fixed sequence and evidence links |

## Implemented and exercised

StagingLimits controls one historical transaction's distinct current keys and the
combined accounted bytes of its pending mutations and all saved map copies/markers.
Defaults are 256 keys and 8 MiB. Checks precede staging/savepoint copies. The key
policy cannot raise the physical WAL maximum. Replacements refund previous current
bytes; tombstones retain key/header charges. Release/rollback update exact usage,
dropping discarded maps before restoring a target. Snapshots, marker lookup and
monotone read/scan observations retain their contracts.

Native/typed begin_with_limits and historical SQL with_session_limits expose policy;
existing entry points use defaults. Native/typed usage accessors and SQL scoped usage
expose accounting. CLI --max-staged-keys and --max-staged-bytes require --history.
The 257th distinct-key error intentionally occurs at mutation rather than COMMIT.

Seven new native tests exercise the physical ceiling even under a higher policy,
unchanged WAL/state after refusal, successful valid-prefix commit, exact byte
boundaries, replacement/delete refunds and coalescing, shared savepoint budgets,
release and rollback refunds, descendants, preserved read-conflict checks, zero-budget
read-only work, drop/checkpoint/reopen cleanup and default 8 MiB refusal before the
16-marker ceiling with large staged rows. Three SQL tests exercise multi-table full
abort without markers, byte refusal at SAVEPOINT and explicit rollback recovery,
commit/reopen preservation and zero-byte read-only policy. Existing native capacity
tests now verify early refusal; existing CLI process tests verify history-only flag
rejection, zero-budget reads/write refusal and positive policy success. Existing
analytical, relational, recovery, corruption, migration and HTTP suites still pass.

## Remaining scope

The budget accounts retained staging payload/header bytes per historical transaction,
not allocator/container slack or process RSS. Read observations/pins, fixed transaction
headers, parser/typed statement inputs, transient inline point-read copies, scan/output
buffers and commit serialization remain separate. Active transaction count, global
admission, catalogs, backing-store open/replay and other engines' staging remain open.
Commit may independently fail WAL byte capacity, retained-history policy or conflicts.

No allocation-failure, disk-full, failed I/O, physical power-loss, browser or macOS/
Windows qualification is claimed. Gate 1 local identity and broader failure/memory
qualification still need work. See the [policy](../operations/HISTORY_STAGING_LIMITS.md),
[ADR 0044](../adr/0044-historical-transaction-staging-limits.md) and
[fixed roadmap](BRADIAN_MASTER_ROADMAP.md).
