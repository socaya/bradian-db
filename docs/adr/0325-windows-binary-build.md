# ADR 0325 — Windows binary build (CI workflow)

## Status

Accepted. Closes G1-PORTABILITY's "Windows binary build in
CI" item on the workflow axis (unsigned binaries only;
signed installers remain ADR 0326).

## Context

G1-PORTABILITY was marked "DONE on code" after ADR 0291
added the `#[cfg(windows)]` branches in `local_access`.
What was still missing was a reproducible way for an
operator to **get a Windows binary** pinned to a given
commit of `main`. The existing `cross-platform.yml`
workflow matrix-tests on `windows-latest` but does not
emit a build artefact.

Per CLAUDE.md, automatic CI triggers remain disabled;
every workflow is manual-only.

## Decision

New workflow at `.github/workflows/windows-binary.yml`
triggered by `workflow_dispatch` (manual). Steps:

1. Checkout the triggered commit.
2. Install pinned stable Rust via `rustup`.
3. Cache the cargo registry + `target/` keyed on
   `Cargo.lock` hash.
4. Record `rustc --version` and the SHA-256 of
   `Cargo.lock` as build-input receipts.
5. `cargo check --workspace --locked`.
6. `cargo test --workspace --locked --no-fail-fast`
   (optional — a workflow input lets an operator skip
   tests if they already ran the full matrix on an
   earlier commit).
7. `cargo build --release --locked` for the two shipped
   binaries: `bradian-admin` and `bradian-db`.
8. Verify both `.exe` artefacts exist (fail the job if
   either is missing).
9. Upload each as a named GitHub artefact with 30-day
   retention: `bradian-admin-windows-x86_64`,
   `bradian-db-windows-x86_64`, plus
   `windows-build-receipt` (rustc version + Cargo.lock
   hash) for cross-referencing against an ADR 0319
   certification manifest.

## Scope

**In scope:**
- A real, manually-triggerable workflow that produces
  Windows release binaries pinned to the invoked
  commit's `Cargo.lock`.
- Build-input receipts (`rustc --version` +
  `sha256 Cargo.lock`) uploaded alongside the binaries
  so the artefacts can be matched against a
  `CertManifest` (ADR 0319).
- Fail-closed on missing `.exe` outputs.
- Cargo registry + `target/` caching keyed on the
  lockfile hash, so a repeat trigger from the same
  commit reuses the cache.

**Out of scope** (dedicated follow-ups):
- Code signing + SmartScreen reputation — ADR 0326.
- MSI / MSIX packaging.
- Auto-trigger on push — explicitly forbidden by
  CLAUDE.md.
- cross-compile from macOS / Linux (mingw) — the
  workflow uses `windows-latest`, not
  `cross-platform.yml`'s matrix, because a real Windows
  runner builds what customers would actually run.
- Multi-architecture (arm64 Windows) — a trivial matrix
  extension when GitHub-hosted arm64 Windows runners
  arrive in GA.

## Consequences

- An operator can click "Run workflow" on the
  `windows-binary` job and download pinned binaries
  within a few minutes.
- The emitted `windows-build-receipt` gives a reviewer
  the exact `rustc --version` and `Cargo.lock` hash that
  produced the artefacts — slots directly into the
  ADR 0319 certification flow.
- G1-PORTABILITY's CI artefact axis is now closed; what
  remains is signing + packaging (ADR 0326).
