# ADR 0321 — POSIX ACL inspection in local_access

## Status

Accepted.

## Context

`local_access` enforced POSIX mode 0o700 + effective-UID
ownership, but POSIX ACLs (via `getfacl` / `setfacl`) can
broaden access beyond those bits. G1-LOCAL-ACCESS called
for ACL inspection as the qualification tail.

## Decision

- New `AclPolicy::{Ignore, StrictReject}` enum.
- New `LocalAccessError::ExtendedAcl { path }` variant.
- New `check_posix_acl(path, policy)` — on Linux, uses
  `libc::getxattr` to probe `system.posix_acl_access`. If
  the attribute is present, surface `ExtendedAcl`. On
  non-Linux platforms the check is a no-op (honestly
  documented; macOS uses NFSv4 ACLs via different APIs).
- New `ensure_private_root_with_acl(path, policy,
  acl_policy)` — composes `ensure_private_root` + the
  ACL check.

## Consequences

- Linux callers can opt in to strict ACL rejection without
  shelling out to `getfacl`.
- macOS / BSD callers keep the pre-ADR behaviour — ACL
  enforcement on those platforms is still an open item
  (captured in the Platform Gaps Plan).
- The check is conservative: ANY `system.posix_acl_access`
  xattr is treated as "extended ACL present". A more
  granular check (parse the ACL blob, accept entries that
  grant only owner RWX) is a future slice.
- 4 cases in `tests/local_access_acl.rs` cover Ignore
  is-noop, Strict pass without ACL, standalone helper both
  policies.
