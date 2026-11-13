# Bradian executed verification — 13 November 2026

## Result

**562 passed, 0 failed, 0 ignored** across the Rust workspace with all declared
features enabled. This includes **307 relational tests**. The workspace check,
strict Clippy across all targets/features and Studio typecheck/build passed.
These are executed Linux checks, not production or cross-platform certification.

Baseline: `23d4f1798960debf6f9e17e06adc2ed829cc307d` (G34). This increment adds twelve
G35 integration tests and preserves the earlier tests. Two process-exit recovery
tests run in isolated integration binaries; shared fixture helpers remain ordinary
test support code.

Environment: Linux x86_64, rustc 1.100.0 stable, Cargo 1.100.0, Node 24.22.1 and npm
11.9.0. Rust dependencies use the committed Cargo.lock with `--locked`. Studio uses
the committed package-lock.json through `npm ci`. Automatic CI remains manual-only.

| Executed command | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 562 passed; 63 target summaries; no failures or ignored tests |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| `npm ci --no-audit --no-fund` in `apps/dap-ui` | Passed |
| `npm run typecheck` in `apps/dap-ui` | Passed |
| `npm run build` in `apps/dap-ui` | Passed; 99 modules transformed |
| `git diff --check` | Passed |

Cargo emits existing unused manifest dependency/field advisories. No compiler or
Clippy diagnostic caused a failure in the final run.

## Implemented increment

[G35](../gates/BRD-G35-live-schema-epoch-rows.md) integrates the G32 row codec with
G23's typed historical SQL. A one-time, explicit capability decision enables BRVE
writes for subsequently begun transactions. Schema definitions, original row bytes
and historical LSNs stay unchanged. Mixed BRRW/BRVE rows decode in point reads,
scans, INSERT/UPDATE/DELETE paths, and AS OF. New envelopes bind to the exact table
ID and immutable schema-registration epoch.

Tests exercise transition status and duplicate prevention; unchanged catalog and
legacy bytes; mixed-format multi-table writes and tombstones; historical point and
scan reads; restart/checkpoint; concurrent opt-in; transactions spanning activation;
G24 backup/restore with inactive G28–G31 DDL records; exact inline capacity and SQL
rollback; retention and below-floor refusal; eight malformed retained-envelope
cases; three invalid activation orderings; four corrupt metadata cases; five torn
activation-frame truncation/retry cases; and process exit without destructors.

## Test-harness correction

An initial full run reported intermittent `Storage(Busy)` in otherwise independent
fixtures, while the focused suite passed. A crash subprocess can temporarily inherit
file descriptors held by other threads until exec closes them. The G34 and G35
process-exit tests now run in dedicated integration binaries, so spawning cannot
inherit unrelated fixture locks. The final full parallel workspace run passed.
No production lock checking was weakened, and no tests were ignored or forced to
run serially to obtain a passing result.

## Remaining work

The active schema remains immutable in the history domain. G34 catalog revisions
cannot yet be activated against that domain. Coordinated catalog/history recovery,
SQL ALTER TABLE and mixed active schema revisions remain open. Full serializable
SQL, production HTAP/distributed SQL and complete enterprise security also remain
outside this increment.

This run does not include browser interaction tests, physical power-loss or disk-full
fault injection, Windows/macOS qualification, long-duration soak tests or production
workload benchmarks. The passing suite establishes the exercised behavior above.
