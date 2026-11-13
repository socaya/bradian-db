# Master Gate 1 — Retained-history admission verification

13 November 2026. Baseline: `83e3605213711b3d1dcce46d4ae15ff7bd911b17`.
Task: G1-MEMORY within fixed Gates 0–9. Gate 1 remains active.

## Executed result

**707 Rust tests passed, 0 failed, 0 ignored**, including **437 relational tests**. Workspace compilation with all features,
strict Clippy across all targets/features and Studio typecheck/build passed. The
roadmap checker verifies 60 unique master requirements and fixed Gates 0–9.
No dependency, lockfile, persistent format or SQL grammar changed. Automatic CI
remains manual-only by owner instruction.

Environment: Linux x86_64, Rust/Cargo 1.100.0 stable, Node 24.22.1 and npm 11.9.0.

| Executed check | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed; existing manifest warnings remain |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 707 passed |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| Studio typecheck and `npm run build` | Passed |
| `python3 scripts/verify_roadmap.py` | 60 unique requirements; fixed gate sequence and evidence links |

## Implemented and exercised

Per-domain HistoryLimits cap logical keys, retained versions and accounted bytes.
Commit admits the whole final staged set before the WAL append. Open rebuilds exact
counters using borrowed backing rows; callers can explicitly raise limits. Native,
typed, SQL, CLI, shared/lifecycle migration and restore paths propagate policy.
Backup verification retains the source policy. DDL preflights both durable records,
and activation recovery checks capacity before catalog mutation.

Pruning now uses a 256-entry deletion worklist with a resumable key cursor, bulk
chain-prefix removal and post-sync counter refunds. It retains baseline values and
tombstones, respects snapshot pins and requests release of unused chain capacity.

Eight new native integration tests cover each budget's atomic whole-batch refusal,
exact reopen/checkpoint accounting, lower-policy refusal, tombstones, pinned pruning
and resumed writes, a constructed interrupted deletion prefix, competing writers,
coalesced/savepoint writes, zero budgets, and multi-key/multi-batch cursor boundaries.
The interruption test removes a final deletion frame from a closed fixture; it does
not claim physical acknowledged-data-loss qualification.

Two SQL integration tests cover unchanged rows/history after quota refusal, backup
and explicit-policy restore, and migration refusal without source modification or
destination publication. Three new unit tests cover checked accounting overflow,
DDL refusal without fencing a healthy owner and recovery capacity refusal before
catalog repair. Existing CLI subprocess tests now exercise all history budget flags,
unsupported-mode refusal, refused committed growth and raised-policy recovery.
Existing relational, analytical, HTTP, corruption, migration and process-exit suites
remain passing.

## Remaining scope

These limits bound accounted retained state, not total RSS or an allocation
reservation. Backing RowStore open/replay happens before derived admission. Catalogs,
active transactions/pins/staging, caches/container slack, temporary verification,
formatted JSON and alternate embedded engines still need controls/global budgets.
Default limits may refuse previously valid large history domains until operators
supply a sufficient explicit policy. Refusal never auto-prunes historical data;
normal incomplete-tail repair remains allowed. Incomplete restore/migration staging
retains its prior explicit cleanup/fresh-destination contract.

This report does not qualify OOM/allocation failure, disk-full, failed sync/rename,
physical power loss, browser or macOS/Windows behavior. Local identity and full
memory-pressure/failure qualification remain open. See the [policy](../operations/RETAINED_HISTORY_LIMITS.md),
[ADR 0043](../adr/0043-retained-history-admission.md) and [fixed roadmap](BRADIAN_MASTER_ROADMAP.md).
