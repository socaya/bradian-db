# Master Gate 1 — Private-root local access verification

14 November 2026. Baseline: `dd78b4a`.
Stable task G1-LOCAL-ACCESS in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**792 Rust tests passed, 0 failed, 0 ignored**. Workspace compilation and
Clippy (`-p bradian-relational -p dap-admin --all-targets -- -D warnings`)
passed. CI remains manual-only. No dependency, lockfile, persistent format,
SQL grammar or default isolation changed.

Environment: Darwin 25.2.0 x86_64, Rust/Cargo stable.
The checkout was restored from published `main` before implementation.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational -p dap-admin` | Passed |
| `cargo test --workspace` | 792 passed, 0 failed, 0 ignored |
| `cargo test -p bradian-relational --lib local_access` | 7 passed |
| `cargo test -p dap-admin` | 20 passed (existing CLI private-root test still green under the delegated helper) |
| `cargo clippy -p bradian-relational -p dap-admin --all-targets -- -D warnings` | Passed |

## Implemented and exercised

The POSIX private-root invariants previously embedded in `dap-admin` now live
in `bradian_relational::local_access`. The helper exposes:

- `PrivateRootPolicy::RequireExisting` and `PrivateRootPolicy::CreateIfMissing`.
- `LocalAccessError` with distinct variants for `Unsupported`, `NotFound`,
  `NotADirectory`, `SymlinkedRoot`, `LooseMode { mode }` and `Io`.
- `ensure_private_root(path, policy)` refusing symlinks via
  `symlink_metadata`, non-directories, any `mode & 0o077 != 0`, and
  creating the directory with mode `0o700` on `CreateIfMissing`.

`dap-admin/src/local_sql.rs::private_root` delegates to the helper and maps
each `LocalAccessError` variant to the same `anyhow` message the CLI used
before, so existing CLI tests (`public_symlink_and_non_directory_roots_are_refused_without_changing_permissions`, mode-0o755 refusal, 0o700 creation)
remain byte-compatible.

Seven new unit tests in `local_access::tests` cover the matrix: existing
private directory accepted, missing root created at 0o700, `RequireExisting`
refuses absent, symlinked root refused (even with 0o700 target), regular
file refused as `NotADirectory`, loose modes 0o750/0o770/0o755/0o705/0o707/0o777
all refused with the exact mode surfaced in the error, and `CreateIfMissing`
idempotent against an already-private root.

## Remaining scope

- Effective-UID ownership of the root and every ancestor (needs `chown` or a
  deterministic ambient identity for testing).
- POSIX ACLs (`setfacl`) and extended attributes — a 0o700 directory can still
  carry an ACL granting access to other principals.
- Parent-path trust against world-writable or symlinked ancestors enabling
  TOCTOU rename.
- Windows file-ACL / SID enforcement (currently returns `Unsupported`).
- Enterprise identity (SSO/mTLS session binding to the local operator).

See [ADR 0053](../adr/0053-private-root-local-access.md) and the
[fixed roadmap](BRADIAN_MASTER_ROADMAP.md). Gate 1 remains active.
