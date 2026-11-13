# Bradian G41 executed verification — 13 November 2026

## Result

**623 passed, 0 failed, 0 ignored**, including **368 relational tests**, across
all Rust workspace features. Workspace compilation, strict Clippy across all targets
and features, and Studio typecheck/build passed. Baseline:
`97aa9fa4f1ad48c3e264a1deb13e96465c0e5c9c`.

Environment: Linux x86_64, Rust/Cargo 1.100.0 stable, Node 24.22.1, npm 11.9.0.
No dependencies, lockfiles or persistent formats changed. Automatic CI remains
manual-only. Existing unused manifest dependency/field advisories remain.

| Executed command | Result |
|---|---|
| `cargo check --workspace --all-features --locked` | Passed |
| `cargo test --workspace --all-features --locked --no-fail-fast` | 623 passed; no failures or ignored tests |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Passed |
| `npm run typecheck` in `apps/dap-ui` | Passed |
| `npm run build` in `apps/dap-ui` | Passed |

## Delivered and verified

G41 adds bounded savepoints, rollback-to and release in native durable MVCC, the
typed adapter and historical SQL sessions. Markers hold staged mutations only; the
original snapshot and conflict observations remain in force. SQL statement errors
with markers require explicit rollback recovery before more work or COMMIT. Without
markers the original whole-transaction abort contract remains unchanged.

Five native tests verify duplicate-name shadowing, nested release, staged insert/
update/delete restoration, zero-WAL operations, identifier and capacity bounds,
missing-marker nonmutation, retention pins, original-snapshot write conflicts,
monotone scan-conflict protection and cleanup on drop/rollback/conflict.

Six SQL integrations verify cross-table selection of committed writes, historical
reads after commit, checkpoint/reopen, failed-state blocking and explicit recovery,
syntax/type/duplicate errors, marker limits and shadowing, original no-marker error
behavior, concurrent key and scan conflicts, callback cleanup and G40 required
defaults. An isolated subprocess test exits without destructors before and after
outer COMMIT; reopen recovers only the intended atomic batch and no reverted work.

The full earlier analytical, relational, migration, backup, corruption, recovery
and HTTP suites pass. The G40 gate example also corrects DESCRIBE TABLE syntax.

## Limits

This is scoped single-process transaction functionality, not persistent network
sessions, nested commits or serializable isolation. Other SQL engines retain their
previous contracts. General SQL, constraint enforcement, concurrent online DDL,
enterprise identity/transport, production HTAP and distributed SQL remain open.
No browser interaction, physical power-loss, disk-full or cross-platform recovery
qualification was run. See the [gap register](BRADIAN_IMPLEMENTATION_GAPS.md).
