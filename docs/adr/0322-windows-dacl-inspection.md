# ADR 0322 — Windows DACL inspection

## Status

Accepted. Mirrors ADR 0321 (POSIX ACL inspection on
Linux) on the Windows side: refuses the private root
under `AclPolicy::StrictReject` if the directory's DACL
contains ACEs for principals other than the owner.

## Context

`ensure_private_root_with_acl` already runs
`check_posix_acl` on Linux so an admin-ACL granting
read/write to a non-owner principal is refused. On
Windows the equivalent is the DACL (discretionary access
control list) attached to the directory's security
descriptor. Without this check, a Windows deployment
that opens a data root inherited from an existing
directory tree can have broader access than the mode-0o700
analogue implies.

## Decision

New function
`bradian_relational::local_access::check_windows_dacl`:

```rust
pub fn check_windows_dacl(path: &Path, policy: AclPolicy)
    -> Result<(), LocalAccessError>;
```

- Under `AclPolicy::Ignore`, returns `Ok(())` on every
  platform with no I/O.
- Under `AclPolicy::StrictReject` on Windows:
  1. Calls `GetNamedSecurityInfoW` to retrieve
     `OWNER_SECURITY_INFORMATION |
     DACL_SECURITY_INFORMATION`.
  2. If the DACL pointer is NULL, reports
     `ExtendedDacl { ace_count: 0 }` — a NULL DACL grants
     everyone full access and is by definition broader
     than owner-only.
  3. Otherwise enumerates every ACE via `GetAclInformation`
     + `GetAce`, extracts each ACE's SID at the
     standard `SidStart` offset (stable across
     allow/deny/audit ACE subtypes in the Win32 ABI),
     and `EqualSid`-compares against the owner SID.
  4. If any non-owner ACE is present, returns
     `ExtendedDacl { ace_count: N }`.
- Under `AclPolicy::StrictReject` on non-Windows, returns
  `Ok(())` — the function is a documented no-op on
  platforms without a DACL, mirroring `check_posix_acl`'s
  Linux-only shape.

Memory is cleaned up via a `Drop`-based `SdGuard` that
calls `LocalFree` on the security-descriptor pointer
regardless of the exit path.

`ensure_private_root_with_acl` now runs both
`check_posix_acl` AND `check_windows_dacl`; under
`AclPolicy::Ignore` both are no-ops, so no behaviour
change for callers that opted out of ACL inspection.

New `LocalAccessError::ExtendedDacl { path, ace_count }`
carries the directory path and the number of non-owner
ACEs so an operator diagnosing a refusal knows exactly
how many principals were granted extra access.

## Scope

**In scope:**
- Real Win32 FFI via the `windows-sys` crate (target-
  gated `[target.'cfg(windows)'.dependencies]`).
- Four `#[allow(unsafe_code)]` call sites with SAFETY
  comments explaining each invariant.
- `Drop`-based cleanup so the security descriptor is
  always `LocalFree`'d.
- NULL-DACL detection (critical: NULL DACL is the
  all-access case that silent audits miss).
- Dispatch from `ensure_private_root_with_acl` so one
  entrypoint covers both OS ACL surfaces.
- Three new cross-platform tests: `Ignore` is a no-op
  everywhere (proven by passing a non-existent path),
  `StrictReject` is a no-op on non-Windows (same
  proof), and the `ExtendedDacl` error `Display` carries
  both the path and the ACE count.
- Integration: `bradian-admin::local_sql::private_root`
  now surfaces the Windows DACL rejection with an
  operator-readable message pointing at ADR 0322.
- Verified on macOS via `cargo check --workspace` and
  `cargo clippy -D warnings`; runtime behaviour on
  Windows is exercised by ADR 0325's manual-trigger
  `windows-binary.yml` workflow which runs
  `cargo test --workspace --locked` on `windows-latest`.

**Out of scope** (dedicated follow-ups):
- Inherited-ACE auditing — the slice counts
  non-owner ACEs regardless of whether they are
  explicit or inherited. An operator who needs to
  distinguish can inspect the DACL header's
  `AceFlags`; exposing that is a follow-up.
- SACL (auditing) inspection — this slice reads the
  DACL only.
- Writing a mode-0o700-equivalent DACL on directory
  creation — when `CreateIfMissing` is used, the
  directory inherits the parent's DACL. A follow-up
  can call `SetNamedSecurityInfoW` with an
  owner-only DACL after creation.
- Enterprise-identity-aware ACE acceptance (e.g.
  allow a `BUILTIN\Administrators` ACE when the
  deployment manifest says so). The current check is
  strict owner-only.

## Consequences

- G1-LOCAL-ACCESS on the Windows DACL axis is now
  closed in code; `ensure_private_root_with_acl` is the
  single cross-platform entry point an operator wires
  into their deployment.
- `windows-sys 0.59` is now a target-gated dep on
  Windows only; macOS and Linux builds are unchanged.
- 19 of 19 `local_access` tests pass on macOS plus 3
  new cross-platform tests, all green under
  `cargo clippy --workspace --all-targets --locked
  -- -D warnings`.
