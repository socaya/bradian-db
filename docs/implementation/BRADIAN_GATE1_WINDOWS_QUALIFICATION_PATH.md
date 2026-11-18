# Gate 1 Windows qualification — closure path (2026-11-18)

**Purpose.** Document the closure path for the Windows
dimension of `G1-FAULTS` and `G1-PORTABILITY` given the
development environment is macOS (Darwin 25.2.0 /
arm64). This is a procedural document: it records what
closes the gate and what has already been prepared.

## 1. What already exists

- **`.github/workflows/cross-platform.yml`** —
  manual-dispatch GitHub Actions workflow authored under
  ADR 0105. Already targets `macos-latest` and
  `windows-latest` runners. Runs `cargo check`,
  `cargo test`, and `cargo clippy --all-targets -- -D warnings`
  on each platform.
- **ADR 0122 `AdvisoryLock` trait + `StdFileLock`
  reference implementation** — uses
  `std::fs::File::try_lock()` which maps to
  `LockFileEx` on Windows (`LOCKFILE_EXCLUSIVE_LOCK |
  LOCKFILE_FAIL_IMMEDIATELY`).
- **`AdvisoryLockError::SharingViolation`** — a
  Windows-only error variant (`#[cfg(windows)]`) for the
  `ERROR_SHARING_VIOLATION` kernel return code.

Gaps on this macOS host:

- No `windows-latest` runner is physically available.
- `cargo check --target x86_64-pc-windows-msvc` requires
  MSVC toolchain + Windows SDK (not available on
  macOS).
- `cargo check --target x86_64-pc-windows-gnu` requires
  mingw-w64 cross-toolchain (installable but doesn't
  qualify runtime Windows behavior — the result is a
  compile-only sanity check).

## 2. Three honest closure paths

### 2.1 GitHub Actions `windows-latest` (Recommended, procurement-free)

The already-committed workflow runs the full test
matrix on a Microsoft-hosted Windows Server 2022 VM.
Invocation from any shell with `gh` installed:

```sh
gh workflow run cross-platform.yml --ref main
```

Or from the GitHub web UI: Actions → "Bradian cross-platform matrix" → Run workflow → main.

**What this closes:**
- Workspace compiles on Windows.
- `AdvisoryLock` trait's Windows path exercises real
  `LockFileEx` / `ERROR_SHARING_VIOLATION`.
- `fault_recovery.rs`, `fault_recovery_extension.rs`,
  `enospc_fault_simulation.rs`, `device_fault_simulation.rs`
  run with Windows file-sharing semantics.

**What this does NOT close:**
- Physical fsync-on-power-loss (still requires hardware).
- Real NVMe/SSD torn-write behavior (still requires
  hardware).

**Evidence to record after the run:** download the
job log, extract per-test pass/fail, commit a doc
analogous to `BRADIAN_GATE1_FAULT_RECOVERY_MACOS_VERIFICATION_2026_11_18.md`
named `BRADIAN_GATE1_FAULT_RECOVERY_WINDOWS_VERIFICATION_<DATE>.md`.

### 2.2 Local UTM / QEMU Windows 11 ARM

- Requires Microsoft Windows 11 Insider Preview ARM64
  ISO + valid license.
- Boot UTM VM, install Rust, clone the repo, run
  `cargo test` on the same test matrix.
- Records the same evidence as §2.1 but against a
  locally-controlled VM.

**Cost:** ~2–4 hours of setup + ongoing resource use.
VM boots a real Windows kernel, so lock semantics +
file-sharing qualify genuinely.

**Not recommended** as the primary path because §2.1 is
free and reproducible.

### 2.3 Wine / CrossOver on macOS

**Not a valid closure path.** Wine reimplements
`LockFileEx` / `ERROR_SHARING_VIOLATION` from scratch;
results qualify Wine's implementation, not Windows.
Documented here only to prevent this being proposed
later.

## 3. Items that remain open after §2.1 closes

- **Physical fsync-on-power-loss.** Requires one of:
  - Hardware power-switch rig with an attached SSD (per
    ADR 0105's "power-loss harness" procurement item).
  - QEMU with libvirt power-interruption injection + a
    libvirt-hosted test lab.
- **Device-level torn writes on real NVMe/SSD.**
  Requires:
  - A block-layer fault-injection kernel module
    (e.g., `drbd-utils` with disconnect, or a custom
    LKM), or
  - Debug-mode NVMe hardware with documented
    partial-write injection.
- Both remain procurement-dependent per CLAUDE.md §2.

## 4. Verification template to fill after §2.1 runs

```markdown
# Gate 1 Fault Recovery — Windows Verification (<DATE>)

## Host
- Runner: windows-latest
- Workflow run: <GitHub Actions run URL>

## Tests
- fault_recovery.rs              — N passed / M failed
- fault_recovery_extension.rs    — …
- enospc_fault_simulation.rs     — …
- device_fault_simulation.rs     — …
- windows_lock::tests            — …

## What this closes
- G1-FAULTS Windows qualification dimension.
- G1-PORTABILITY Windows source-level compile.
- ADR 0122 StdFileLock's LockFileEx path.

## What this does NOT close
- fsync-on-power-loss (hardware).
- Device-level torn writes (hardware).
```

Push the filled template to
`docs/implementation/BRADIAN_GATE1_FAULT_RECOVERY_WINDOWS_VERIFICATION_<DATE>.md`
to close the gate.

## 5. Suggested next action

Run from the repo root on any shell with `gh`
authenticated:

```sh
gh workflow run cross-platform.yml --ref main
gh run watch                          # or: gh run list
```

Once the Windows job completes (typically ~15–25 min),
download the log and fill out §4's template.
