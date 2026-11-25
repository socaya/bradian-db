# ADR 0326 — Signed portable Windows installer

## Status

Accepted. Closes G1-INSTALL on the **packaging + signing
workflow** axis. The signing *certificate / key value*
itself is an operator-supplied secret, not a repository
asset; the workflow degrades gracefully to UNSIGNED
output when the secret is unset so a developer can still
produce a distributable ZIP without procurement in the
critical path.

## Context

ADR 0325 ships bare `bradian-admin.exe` + `bradian-db.exe`
as workflow artefacts. A customer who downloads those
has no provenance trail: no manifest of what's in the
package, no detached signature they can verify against
the shipping key, no SmartScreen-usable packaging
container.

Per CLAUDE.md: automatic CI triggers remain disabled;
every workflow is manual-only. Per the "never embed
secret tokens in bundles" security rule: signing keys
live in GitHub repository secrets, not in the repo.

## Decision

New workflow at `.github/workflows/windows-installer.yml`
triggered by `workflow_dispatch` with a required
`version_tag` input. Pipeline:

1. Checkout, install pinned stable Rust, cache cargo.
2. `cargo build --release --locked` for `bradian-admin`
   and `bradian-db`.
3. Stage a portable tree
   `bradian-${version_tag}-windows-x86_64/` containing:
   - `bradian-admin.exe`
   - `bradian-db.exe`
   - `README.md`, `LICENSE` (if present at repo root)
   - `rustc-version.txt`
   - `cargo-lock.sha256`
   - `SHA256SUMS` — computed in the workflow.
4. If the secret `WINDOWS_SIGNING_KEY_PEM` is provided,
   sign `SHA256SUMS` with RSA-SHA256-PSS via the
   runner's bundled OpenSSL and record the key ID
   (`WINDOWS_SIGNING_KEY_ID`) in `SIGNATURE_STATUS.txt`.
   If the secret is unset, the workflow writes
   `UNSIGNED` into `SIGNATURE_STATUS.txt` and the
   artefact name is tagged `-unsigned`.
5. Compress the staged tree into
   `bradian-${version_tag}-windows-x86_64.zip`.
6. Verify the ZIP contains every required entry
   (`bradian-admin.exe`, `bradian-db.exe`, `SHA256SUMS`,
   `SIGNATURE_STATUS.txt`) — fail the job if any are
   missing.
7. Upload the ZIP as a GitHub artefact with 90-day
   retention. Artefact name carries the signed/unsigned
   status so a reviewer can distinguish them without
   downloading.

## Scope

**In scope:**
- Real packaging of both shipped binaries into a
  portable ZIP with manifest and build-input receipts.
- Real OpenSSL RSA-SHA256-PSS signing of `SHA256SUMS`
  **when the operator provides a key via repository
  secrets**. No key is embedded in the repository.
- Graceful UNSIGNED fallback so a developer can reproduce
  the layout + the signature slot without procurement.
- 90-day artefact retention; the longer window reflects
  a release-candidate workflow rather than the 30-day
  CI-build retention of ADR 0325.
- Structural validation (every required entry is in the
  ZIP) before upload.

**Out of scope** (dedicated follow-ups):
- MSI / MSIX authoring (requires WiX toolset + an
  installer schema this ADR does not design).
- SmartScreen reputation — the EV certificate purchase
  is procurement, and SmartScreen reputation has to be
  earned over time regardless of code.
- Auto-update manifest (`updates.json`, Sparkle-style
  signed feed).
- Multi-architecture Windows builds (arm64) — trivial
  matrix extension once runners exist.
- GPG-signed detached signatures (`SHA256SUMS.asc`) — a
  later slice can emit both under a secrets-fed key.

## Consequences

- An operator can click "Run workflow", supply a version
  tag, and download a complete signed (or clearly-marked
  unsigned) portable Windows ZIP.
- The SHA256SUMS + detached signature flow is standard
  across every Linux distribution's release process; a
  reviewer with the shipped public key can verify the
  artefact locally with nothing but `openssl dgst
  -verify`.
- Combined with ADR 0325's build-input receipt and
  ADR 0319's certification manifest, a reviewer has
  three independent provenance signals for a shipped
  Windows binary: lockfile hash, build-input receipt,
  signed SHA256SUMS.
- G1-INSTALL on the workflow axis is closed. Signing-key
  procurement, SmartScreen reputation, and MSI/MSIX
  authoring remain operator / procurement work.
