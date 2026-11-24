# ADR 0320 + 0321 + 0322 + 0323 + 0324 + 0325 + 0326 — Gate 1 qualification infrastructure

## Status

Accepted as scope declaration. Each item stays `OPEN` in
the Platform Gaps Plan. These items close the final Gate 1
qualification tasks per the fixed master roadmap and
require infrastructure (hardware, OS, external systems)
rather than code slices.

## Context

The fixed master roadmap (`BRADIAN_MASTER_ROADMAP.md`)
lists six Gate 1 qualification tasks. Four are documented
in-code (`G1-CATALOG`, `G1-STORAGE`, `G1-TRANSACTION`,
`G1-CLI`). The remaining four (`G1-MEMORY`,
`G1-LOCAL-ACCESS`, `G1-FAULTS`, `G1-REFERENCE`,
`G1-PORTABILITY`, `G1-INSTALL`) each have an "open"
qualification tail documented below.

## Decision

Design notes per item so a future owner can pick them up:

### ADR 0320 — G1-MEMORY qualification tail

HistorySqlEngine does not yet sample its own
`DurableTypedStore`-held retained-history keys into the
workspace budget. The sample point is at every statement
enter; the call is
`WorkspaceUsage::with_durable_typed(&self.store)`.
Memory-pressure failure qualification needs a probe: induce
per-engine memory growth, verify `GlobalBudget::check`
rejects the next statement with `SqlError::Resource(_)`.

### ADR 0321 — POSIX ACL inspection

`bradian_relational::local_access` enforces mode 0o700 and
effective-UID ownership. POSIX ACLs (`getfacl` / setfacl`)
are ignored. Harden by: after the mode check, call
`acl_get_file` (libc) and reject any extended ACL entry
that broadens the permission beyond 0700 for the owner.
`libc` dep already in the workspace.

### ADR 0322 — Windows ACL

Requires Windows runner. Design: wrap private-root check
with a `#[cfg(windows)]` branch that uses `windows-sys`
crate to query the security descriptor and reject any ACE
that permits access outside the owning user. Not a code
slice we can run in an autonomous macOS session.

### ADR 0323 — Enterprise identity

Kerberos / AD / LDAP integration. New workspace crate
`bradian-identity` depending on external directories.
Infrastructure dependency; out of scope for autonomous
completion.

### ADR 0324 — Device-level torn-write lab

Hardware setup: an external USB / iSCSI block device with
power control or a QEMU scratch image that supports
`virtio-blk` with injected tears. Not a code slice.

### ADR 0325 — Installation qualification

Signed portable installers across macOS / Windows / Linux
with a test matrix. Requires three-OS CI with installer
runners. Currently CI is disabled by owner directive — this
is a CI infrastructure dependency.

### ADR 0326 — Signed installers

Code-signing certificates for macOS (Apple Developer),
Windows (EV code signing), Linux (GPG). Requires issued
certificates from the owner — infrastructure dependency.

## Consequences

- Six qualification tasks remain OPEN with explicit
  infrastructure dependencies. They are not blockers that
  can be closed by code alone.
- The Gate 1 code implementation (catalog, storage,
  transaction, SQL conformance, authorization, fault
  injection) is substantially shipped per ADRs 0001–0288.
  Gate 1 **acceptance** awaits the qualification
  completion items above.
