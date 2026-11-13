# Master Gate 1 — Backing RowStore map verification

13 November 2026. Baseline: `7de0e9a689bb781304b29c4f64c25bbb0e0cb7ed`.
Stable task G1-MEMORY in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**727 Rust tests passed, 0 failed, 0 ignored**, including **457 relational tests**.
Workspace compilation with all features, strict Clippy across all targets/features,
Studio typecheck/build and the fixed-roadmap check passed. CI remains manual-only.
No dependency, lockfile, persistent format, SQL grammar or default isolation changed.

Environment: Linux x86_64, Rust/Cargo 1.100.0 stable, Node 24.22.1 and npm 11.9.0.

| Executed check | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed; existing manifest warnings remain |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 727 passed |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| Studio `npm run typecheck` and `npm run build` | Passed |
| `python3 scripts/verify_roadmap.py` | 60 unique requirements, fixed sequence and evidence links |

## Implemented and exercised

RowStoreLimits/RowStoreUsage bound live-row and committed WAL-delta map counts/bytes.
Defaults are 2000000 live rows and delta keys, with 256 MiB for each map's accounted
bytes. Snapshot rows are admitted before copying; replay admits a complete validated
frame's final changes before application. Existing default opens, including higher
SQL/history backing stores, use this policy. Native open/restore allow explicit
limits; policy is immutable per owner and is not persisted.

Put/delete/batch commit calculate final coalesced map state under the mutation mutex
before WAL encoding/append. Removing old affected entries before copying final entries
avoids over-budget map peaks during swaps. Quota refusal leaves WAL/rows/LSN/usage
unchanged and keeps the healthy owner usable. Delta tombstones count until checkpoint.
Only successful checkpoint publication and WAL truncation/sync release delta charges.

Map admission completes before missing-index repair or incomplete-tail trimming.
Refused open does not truncate WAL. A lower policy must fit the checkpoint and every
replayed commit prefix. Backup verification uses source policy and borrowed recovered
rows; explicit-policy restore verifies its stage before destination publication.

Nine new integration tests cover each budget's whole-batch refusal with unchanged
WAL/LSN/rows/usage, exact replacement/delete/tombstone accounting, checkpoint refunds,
repeated keys and cross-key swaps, exact checkpoint reopen and refused index repair,
all four replay limits with an incomplete tail left untouched, higher-policy recovery
and normal tail repair, lower-policy prefix refusal followed by explicit checkpoint,
concurrent writer admission, backup/restore destination refusal and zero policy.
One new unit test verifies accounting overflow reports a resource error instead of
wrapping. Existing atomic batch, snapshot/index corruption, recovery, historical SQL,
DDL, migration, backup, analytical, CLI and HTTP suites still pass.

## Remaining scope

These are per-map accounted-state limits, not process RSS, allocator reservations or
a global pool. Page/frame decoding, metadata/directory inputs, index/checkpoint/backup
audit buffers, caches, allocator/container slack and higher-level catalogs/history/
transaction pins remain separate. SQL/history APIs currently use backing defaults;
their other budget flags do not configure these native backing limits. A previously
valid larger store may require explicit native limits, and a lower policy can refuse
an earlier replay prefix even when the final live map is small.

No automatic checkpoint, pruning or data discard runs on quota refusal. Restore
refusal preserves the prior incomplete-stage cleanup/fresh-destination contract.
This report does not qualify allocation failure, OOM, failed I/O, disk-full, physical
power loss, browser or macOS/Windows operation. Global admission, local identity and
broader failure qualification remain open. See the [policy](../operations/ROW_STORE_LIMITS.md),
[ADR 0045](../adr/0045-backing-row-map-admission.md) and [fixed roadmap](BRADIAN_MASTER_ROADMAP.md).
