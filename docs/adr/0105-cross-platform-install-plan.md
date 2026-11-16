# ADR 0105 — Cross-platform install qualification plan (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16 as a planning deliverable.**

## 1. Honest scope

Shipped in-repo:
- `.github/workflows/cross-platform.yml` — manual CI matrix
  across macOS + Windows runners. Same cargo check / test /
  clippy as the Linux baseline.
- `docs/operations/BRADIAN_INSTALL_MANUAL.md` — three-OS
  source-build install guide with honest "this is NOT a
  signed installer" callouts.

## 2. Out of scope (OWNER / PROCUREMENT)

- **Actual Windows / macOS CI runs** — requires the owner to
  enable the manual workflow dispatch and host runners.
- **Signed installers** (MSI, DMG, deb, rpm) — a signing
  identity + release infrastructure decision belongs to the
  owner.
- **Automatic updates** — needs release-channel strategy.
- **Service integration** (systemd, launchd, Windows service)
  — out-of-scope until the install channel is chosen.
- **Windows advisory-lock qualification** — our rows.lock
  convention is a POSIX advisory lock; `LockFileEx`
  semantics on Windows differ in subtle ways (file-sharing
  modes). G1-PORTABILITY remains open.

## 3. Compliance

1. Cross-platform workflow file exists and is syntactically
   valid YAML.
2. It is `workflow_dispatch` only (matches CLAUDE.md
   automatic-CI-disabled convention).
3. Install guide covers Linux / macOS / Windows paths at the
   source-build level.
4. Every "not shipped" item is explicitly named.
