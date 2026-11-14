# Master Gate 1 — Effective-UID local-access verification

14 November 2026. Baseline: `eb159ce`.
Stable task G1-LOCAL-ACCESS in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**840 Rust tests passed, 0 failed, 0 ignored**. Clippy clean on
`bradian-relational --all-targets` and `dap-admin --all-targets`. CI
remains manual-only. No persistent format, wire protocol or SQL grammar
change. The crate-level unsafe policy was relaxed from `forbid` to
`deny` to allow the single documented `libc::geteuid()` call site.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational -p dap-admin` | Passed |
| `cargo test --workspace` | 840 passed, 0 failed |
| `cargo test -p bradian-relational --lib local_access` | 10 passed (+3 new) |
| `cargo clippy -p bradian-relational -p dap-admin --all-targets -- -D warnings` | Passed |

## Implemented and exercised

`LocalAccessError::WrongOwner { owner_uid, expected_uid }` is added as a
distinct variant. `ensure_private_root(path, policy)` now compares the
root's owner UID (via `MetadataExt::uid()`) against the effective UID
returned by `libc::geteuid()`. A mismatch returns `WrongOwner` after
the existing symlink, directory and mode checks have passed.

`ensure_private_root_owned_by(path, policy, expected_uid)` exposes the
check with an explicit expected UID so negative-path tests can prove
the refusal without requiring `chown` or root privileges.

Check ordering is explicit: symlink → directory → mode → owner. The
new `mode_refusal_takes_precedence_over_uid_mismatch` test proves mode
refusal is reported first, so operators fix permissions before
ownership.

The `dap-admin` CLI's `private_root()` handler maps `WrongOwner` to an
anyhow message naming the actual and expected UIDs; existing CLI tests
still pass under the amended helper.

Three new unit tests cover (1) default entry point accepting a
process-owned directory, (2) deterministic refusal against a clearly
distinct expected UID (`u32::MAX`), (3) mode refusal taking precedence
over owner mismatch.

## Remaining scope

- POSIX ACLs and extended attributes.
- Parent-path trust against world-writable or symlinked ancestors
  (owner decision: canonicalize-once vs per-component).
- Windows file-ACL / SID enforcement.
- Enterprise identity (SSO/mTLS session binding).

See [ADR 0057](../adr/0057-private-root-effective-uid.md),
[ADR 0053](../adr/0053-private-root-local-access.md) and the
[fixed roadmap](BRADIAN_MASTER_ROADMAP.md).
