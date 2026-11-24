# ADR 0291 — Windows code path for `local_access`

## Status

Accepted.

## Context

G1-PORTABILITY called for Windows installation + lock
lifecycle. The full test (actual Windows CI runner) is
infrastructure (ADR 0325). The CODE path for Windows is
in-scope: the `local_access` module previously returned
`LocalAccessError::Unsupported` on every non-Unix target,
which breaks any Windows caller that tries to open a
private root.

## Decision

- `ensure_private_root` now has three branches:
  - `#[cfg(unix)]` — existing POSIX enforcement (mode
    0o700, effective-UID ownership).
  - `#[cfg(windows)]` — NEW: delegates to
    `windows_private_root(path, policy)` which:
    - Rejects symlinks.
    - Rejects non-directories.
    - Under `CreateIfMissing`, creates the directory.
    - Does NOT yet check Windows ACLs (that is ADR 0322).
  - `#[cfg(not(any(unix, windows)))]` — still
    `Unsupported` (truly unknown OS).
- `ensure_private_root_owned_by` adds the same Windows
  branch (the `expected_uid` parameter is ignored on
  Windows since the Unix UID concept doesn't apply).
- `check_parent_path_trust` gains a Windows branch that
  returns `Ok` if the path exists (world-writable bits
  don't have a direct Windows analogue; the full ACL-based
  check is ADR 0322).

## Consequences

- Code builds on Windows targets when cross-compiled with
  the appropriate toolchain. Cross-compile verification on
  macOS requires `x86_64-w64-mingw32-gcc` which is not
  installed in the default `rust-std` target; the actual
  Windows binary build is a CI infra task (ADR 0325).
- The Windows path explicitly does NOT claim parity with
  Unix invariants — ADR 0322 (Windows ACL inspection) is
  the follow-up that elevates it.
- 4 tests in `tests/windows_code_path.rs`: one
  platform-agnostic smoke (always runs), one Unix
  regression (runs on Unix), two Windows-only (run on
  Windows CI once available). Current macOS run executes
  the 2 non-Windows tests.
