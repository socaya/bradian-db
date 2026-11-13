# Master Gate 1 — SQL CLI output verification

13 November 2026. Baseline: `9e0f62081d193f270d7bdb7e63579a4fca817538`.
Stable task G1-MEMORY in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**762 Rust tests passed, 0 failed, 0 ignored**, including **487 relational tests**.
Workspace compilation with all features, strict Clippy across all targets/features,
Studio typecheck/build and fixed-roadmap checks passed. CI remains manual-only.
No dependency, lockfile, persistent format, SQL grammar or default isolation changed.

Environment: Linux x86_64, Rust/Cargo 1.100.0, Node 24.22.1 and npm 11.9.0.

| Executed check | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed; existing manifest warnings remain |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 762 passed |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| Studio `npm run typecheck` and `npm run build` | Passed |
| `python3 scripts/verify_roadmap.py` | 60 unique requirements, fixed sequence and evidence links |

## Implemented and exercised

Native SQL CLI output now borrows result columns/rows/cells and serializes directly
into one bounded line buffer, without another owned JSON result tree. Fixed-size
numeric/UUID conversions remain small temporaries. --max-output-bytes applies to
default, --table and --history modes, defaulting to 16777216 bytes per statement,
including metadata/type tags, escaping, statement ordinal and newline.

Checks precede each buffer extension; only a complete admitted line is written and
flushed. Quota refusal emits none of the failed line, retains earlier complete lines
and stops the script. Scoped callbacks discard pending transactions. Completed
autocommits/COMMITs retain their existing durability independent of output success.
Actual output-device write/flush errors retain statement context and can still lose
an acknowledgement or expose partial bytes.

Four new unit tests compare exact bytes against the previous owned-map oracle for
all value types and commands, check exact/newline/zero boundaries and unchanged prior
output, charge escaped-text amplification, and exercise write/flush errors. One new
CLI process test verifies default/scoped output budgets, complete-line retention,
scoped rollback and durable autocommit despite zero-budget output refusal. Existing
history process tests now verify exact output limits and staged-work rollback after
quota refusal. Both canonical and legacy executable help exposes the flag. Existing
wide-number output, analytical, relational, recovery, corruption, backup, migration,
CLI and HTTP suites still pass.

## Remaining scope

The budget bounds encoded bytes per line, not decoded results, allocator/vector
capacity slack, cumulative script output, other CLI commands or HTTP/Studio responses.
Query and storage limits remain independent. Directory/I/O/cache/decoded metadata
workspace, catalogs, aggregate transactions/pins, alternate engines and global
admission remain open.

No atomic OS output, exactly-once acknowledgements, allocation-failure/OOM,
cancellation, disk-full, physical power-loss, browser or macOS/Windows qualification
is claimed. Gate 1 remains active; see the [policy](../operations/SQL_OUTPUT_LIMITS.md),
[ADR 0049](../adr/0049-bounded-sql-cli-output.md) and [fixed roadmap](BRADIAN_MASTER_ROADMAP.md).
