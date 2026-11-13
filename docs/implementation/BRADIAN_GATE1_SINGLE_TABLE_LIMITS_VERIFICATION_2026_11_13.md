# Master Gate 1 — Single-table snapshot and result limits verification

13 November 2026. Baseline: `5fadcfe39f2376c6db0405bbe7b0d245d99fc3dc`.
Task: G1-MEMORY within the fixed master sequence. Gate 1 remains active.

## Executed result

**694 Rust tests passed, 0 failed, 0 ignored**, including **424 relational tests**.
Workspace compilation, strict Clippy across all targets/features and Studio
typecheck/build passed. The roadmap checker still verifies all 60 master IDs and
fixed Gates 0–9. No dependency, lockfile, persistent format or SQL grammar changed.

Environment: Linux x86_64, Rust/Cargo 1.100.0 stable, Node 24.22.1 and npm 11.9.0.
Automatic CI remains manual-only by owner instruction.

| Executed command/check | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 694 passed |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| Studio `npm run typecheck` and `npm run build` | Passed |
| `python3 scripts/verify_roadmap.py` | 60 unique requirements; fixed gate sequence and source links |

## Implemented and verified

Earlier SqlEngine autocommit SELECT and scoped single-table SQL sessions now use
explicit/default controls. Raw row images capture their LSN under the same mutex and
check copy budgets before allocation. BEGIN bounds its complete encoded/decoded
snapshot, preserving original visibility rather than taking a partial image.
Transaction SELECT merges borrowed snapshot/staged maps without cloning the full
snapshot first. Existing native administrative compatibility methods remain explicit
and are not SQL fallbacks. The one schema metadata key is excluded from row copies;
unexpected namespaces and mismatched row keys are still validated and rejected.

Projection in both earlier and historical SQL now checks returned rows/bytes and
projected-cell work before copying values. Duplicate text columns cannot amplify a
small input into an unbounded result. The policy applies to point-query output too.
CLI budget flags now apply to default, --table and --history modes, replacing the
previous --history-only restriction because all advertised modes enforce them.

Eight new raw/native/SQL tests verify exact raw copy boundaries and consistent LSNs;
decoded snapshot admission failure; compatibility methods; borrowed merge equivalence
with the previous oracle; stable owned snapshots and conservative conflicts;
corruption checks for unexpected/wrong-table keys; autocommit query limits and
pre-read binding; BEGIN/later-query abort without staged publication; repeated-column
byte/work refusal; cancellation and fresh-control recovery. One new historical test
verifies the same projection-amplification failure drops staged work and pins. One
new CLI subprocess test verifies zero limits reject default/scoped scans or BEGIN,
leave persisted data unchanged, and exact positive budgets work. Earlier historical
CLI assertions were updated for the now-enforced all-mode flag contract.

The complete earlier analytical, relational, HTTP, backup, migration, crash,
corruption and query-window suites pass. Native legacy transaction semantics remain
single-table/full-abort; no savepoints, ordering or historical syntax are silently
added to that grammar.

## Remaining limits

These are per-stage accounted buffer/work limits, not a combined RSS reservation.
Resident RowStore/catalog/version maps, open/replay, allocator/container slack,
formatted JSON output, alternate embedded engine paths and global admission remain
outside this slice. One decoded candidate row is transient workspace before charging.
Legacy BEGIN may reject a table larger than policy even when subsequent reads would
be point lookups. The metadata key consumes examined-key work but is not returned.
Work measures keys or projected cells, not elapsed CPU time. Cancellation is cooperative
at read/copy/projection boundaries, not inside DDL/commit, mutex waits or filesystem
I/O. Existing lazy table metadata initialization remains part of store open.

Gate 1 is not accepted by this report. Local identity and I/O/platform qualification
also remain open; no physical power-loss, disk-full, browser or cross-platform run
is claimed. See [fixed roadmap](BRADIAN_MASTER_ROADMAP.md) and the
[single-table policy](../operations/SINGLE_TABLE_QUERY_LIMITS.md).
