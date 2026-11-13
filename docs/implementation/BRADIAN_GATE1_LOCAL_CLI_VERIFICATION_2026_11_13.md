# Master Gate 1 — Native local SQL CLI executed verification

13 November 2026. Baseline: `388acc33340045b4b3ce99283c5785145c07e71f`.
This is task evidence within fixed Master Gate 1, not a new gate or gate acceptance.

## Result

**672 Rust tests passed, 0 failed, 0 ignored**, including **403 relational tests**
and **14 new operator CLI/unit tests**. Workspace compilation, strict Clippy across
all targets/features and Studio typecheck/build passed. The roadmap checker maps
all **60 master requirement IDs** exactly once to fixed Gates 0–9.

Environment: Linux x86_64, Rust/Cargo 1.100.0 stable, Node 24.22.1 and npm 11.9.0.
No persistent format or third-party package version changed. Cargo.lock gains the
operator crate's internal relational dependency and its existing tempfile test
package dependency. Automatic CI stays manual-only by owner instruction.

| Executed command/check | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 672 passed |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| `cargo test -p dap-admin --all-features --locked` | Final CLI recheck passed |
| `cargo clippy -p dap-admin --all-targets --all-features --locked -- -D warnings` | Final CLI recheck passed |
| Studio `npm run typecheck` and `npm run build` | Passed |
| `python3 scripts/verify_roadmap.py` | 60 unique requirements, fixed Gate 0–9 sequence, source links exist |
| `cargo install --path crates/dap-admin --bin bradian-db --locked --offline --debug --target-dir target --root /tmp/bradian-gate1-install` | Installed canonical development executable |
| Installed executable create/insert/commit/reopen | Separate processes returned the persisted expected value |

## Implemented and tested

The canonical bradian-db and retained dap-admin executable share the existing
operator command implementation. Native sql embeds the relational engines, reads
bounded UTF-8 execute/file/stdin scripts, preserves quoted semicolons and escaped
quotes, and emits one typed JSON result per successful statement. Wide integers,
decimal coefficients and timestamp microseconds preserve exact values as strings.

Four unit tests verify splitting, comments/unterminated/empty script refusal,
statement-count/size and input/UTF-8 bounds, and lossless result encoding. Ten real
executable integration tests verify canonical/legacy commands; persisted DDL/DML;
explicit single-table commit/rollback; incomplete/failed transaction rollback;
stdin and Unicode; stop-on-error with prior-commit preservation; diagnostics without
SQL echo; pre-open malformed input refusal; private mode/final-symlink/non-directory
root refusal; conflicting mode refusal; process lock contention; and explicit
historical read validation, expressions, ordering and restart persistence.

Process-spawning fixtures are serialized within their test binary so transient
fork/exec inheritance cannot retain another fixture's lock during a reopen. The
lock-contention test still holds a real parent catalog lock while the child tries
to open it and verifies refusal. Earlier recovery, corruption, analytical, HTTP,
SQL, migration and backup tests remain green.

## Roadmap reconciliation and limits

The fixed master roadmap, 60-row traceability matrix, source/format inventory and
current threat model replace the mistaken interpretation of BRD-G1–BRD-G44 as
accepted master gates. Historical files and protocol names remain intact. Global
ordering, cluster identity and authorization protocols remain unfrozen pending the
specified experiments. Gate 0 evidence is reviewable; this is not independent
architecture or security sign-off.

Gate 1 remains active. The CLI is Unix-only and its private-mode/final-symlink
checks are not full OS-owner/ACL/hostile-path protection or library-wide SQL
identity. Default and explicit transaction modes retain their existing bounded
engine contracts. Historical mode requires an existing migrated root. Script input
limits do not bound underlying scans, version/catalog memory or output volume.
Staged statement output is not a durable acknowledgement; output can fail after
COMMIT. Prior explicit/autocommit transactions survive later errors.

The Linux debug development installer is exercised; release signing, packaging,
Windows/macOS recovery, physical power-loss, disk-full and failed sync/rename
qualification are not supplied here. No full Gate 1 closure, later gate advancement,
secure remote SQL service or production DBMS certification is claimed. See the
[fixed roadmap](BRADIAN_MASTER_ROADMAP.md) for the next kernel/runtime/access/fault
acceptance work before progressing to Gate 2.
