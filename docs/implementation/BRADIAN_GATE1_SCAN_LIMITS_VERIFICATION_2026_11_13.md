# Master Gate 1 — Historical query scan limits verification

13 November 2026. Baseline: `4ec46b0562c6df5e451ef684bc2fc787c94aa736`.
This is evidence for task G1-MEMORY within fixed Master Gate 1, not another gate.

## Result

**684 Rust tests passed, 0 failed, 0 ignored**, including **415 relational tests**.
Workspace compilation, strict Clippy across all targets/features and Studio
typecheck/build passed. The roadmap checker retains all 60 master requirement IDs
exactly once under fixed Gates 0–9. No dependency, lockfile or persistent format changed.

Environment: Linux x86_64, Rust/Cargo 1.100.0 stable, Node 24.22.1 and npm 11.9.0.
Automatic CI remains manual-only by owner instruction.

| Executed command/check | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 684 passed |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| Studio `npm run typecheck` and `npm run build` | Passed |
| `python3 scripts/verify_roadmap.py` | 60 unique requirements; fixed gate sequence and evidence links |

## Implemented and verified

Historical SQL scans use a selected-table BRPK prefix and merge staged mutations
without cloning all domain rows or the staged map. Controlled scans enforce visible
row, accounted encoded/decoded buffer-byte and key/version-work limits. Cancellation
is shared across control handles. Ordered queries lower the input-row ceiling before
cloning. Resource failures are distinct from corruption and return no partial result.

Five new raw history tests compare prefix/historical/staged results with the existing
full-scan oracle; verify exact row/byte/work boundaries; show unrelated tables do not
consume the selected-prefix budget; bound old-version searches; reject invalid prefix
capacity; retain scan conflicts and retention pins across failed scans/savepoints;
and verify cancellation, retry with a fresh control, no WAL writes and pin cleanup.

Five new historical SQL tests verify selected-table current/AS OF limits; full abort
and savepoint recovery after resource failure; decoded immutable-default expansion
beyond encoded input size; exact typed resource error classification; per-query
counter resets; cross-thread cancellation flags with full rollback still usable;
and conflict retention after a failed LIMIT 0 scan and concurrent commit. Two meter
tests prove cancellation after accounting has started and overflow-safe accounting.
The existing CLI subprocess tests now additionally verify flags require --history,
each zero budget rejects a scan without publishing earlier staged changes, and exact
one-row/two-work-step limits allow a valid query.

All earlier query-window, epoch, process-exit, backup, migration, analytical, HTTP,
rollback and corruption suites also pass. Existing administrative typed/raw scan
APIs preserve their compatibility behavior; SQL directly uses controlled methods
and cannot fall back to them. No migration/backup data-size contract is silently
replaced by a query-policy ceiling.

## Remaining Gate 1 work

Limits account encoded and decoded query buffers separately, including logical
row/vector headers and owned text capacity. They do not reserve total RSS or include
allocator/container slack, resident store/catalog/version maps, all schema work,
output serialization or older SQL/native compatibility snapshots. One bounded-inline
candidate row is decoded before charging the typed result buffer. Work units count
merged keys/inspected versions; they are not CPU-time deadlines. Cancellation checks
occur at cooperative scan/decoding and bounded sort boundaries, not inside mutex
waits or filesystem I/O. A cancelled handle stays cancelled after ROLLBACK TO; full
ROLLBACK releases staged work/pins and a fresh session/control permits new queries.

G1-MEMORY is PARTIAL: resident admission/eviction, legacy paths, output limits and
complete cancellation coverage still need work. Local identity and failed-I/O/platform
qualification remain other Gate 1 blockers. No new gate advancement, streaming disk
scan, global process-memory guarantee, browser/physical power-loss test or production
DBMS qualification is claimed. See [fixed roadmap](BRADIAN_MASTER_ROADMAP.md) and
[operational limits](../operations/HISTORY_QUERY_LIMITS.md).
