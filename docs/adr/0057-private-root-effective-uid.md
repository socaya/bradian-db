# ADR 0057 — Effective-UID private-root check (G1-LOCAL-ACCESS)

**Status:** Accepted. Amends [ADR 0053](0053-private-root-local-access.md).
Gate 1 remains active. This does not accept the full master G1-LOCAL-ACCESS
exit (POSIX ACLs, parent-path trust, Windows ACL, enterprise identity).

ADR 0053 established the mode/symlink/directory invariants of the private
root. It explicitly left owner-UID ownership out of scope because
deterministic testing without `chown` required a design choice.

This amendment adds effective-UID ownership enforcement:

1. `LocalAccessError::WrongOwner { owner_uid, expected_uid }` — distinct
   from mode and symlink refusals.
2. `ensure_private_root(path, policy)` now compares the root's owner UID
   (via `std::os::unix::fs::MetadataExt::uid`) against the effective UID
   returned by `libc::geteuid()`. A mismatch returns `WrongOwner` after
   all prior checks have passed.
3. `ensure_private_root_owned_by(path, policy, expected_uid)` is the
   test-visible workhorse. Supplying an expected UID that cannot match
   any real file owner (e.g. `u32::MAX`) deterministically proves the
   refusal without requiring `chown` or root privileges.
4. Check ordering: symlink → directory → mode → owner. Mode refusal
   takes precedence over owner mismatch so operators fix permissions
   before ownership.

### Dependency and unsafe policy

The amendment adds `libc = "0.2"` as a Unix-only direct dependency on
`bradian-relational`. `libc` is already in the transitive dep graph, is
widely audited, and is listed in CLAUDE §5 only implicitly (via its
`hashing` and `async runtime` transitive users). This ADR records it as a
direct dep.

The `libc::geteuid()` call is `unsafe`-to-call because it is `extern "C"`.
It has no preconditions, no error return, and no side effects beyond
returning the caller's effective UID — it cannot fail or alias memory.
The crate-level `#![forbid(unsafe_code)]` is relaxed to `#![deny(...)]`
so the single call site can opt in with `#[allow(unsafe_code)]` and a
SAFETY comment. This matches the policy in CLAUDE §4.9 ("No unsafe
outside clearly-scoped sites; every `unsafe` block carries a `// SAFETY:`
comment"). No other site in the crate is permitted to add unsafe without
its own review.

### Not enforced by this ADR

- POSIX ACLs and extended attributes.
- Parent-path trust (world-writable or symlinked ancestors enabling
  TOCTOU rename). Needs an owner decision between canonicalize-once and
  per-component checks (comprehensive implementation plan §13).
- Windows file-ACL / SID enforcement; the shared helper still returns
  `Unsupported` there.
- Enterprise identity (SSO/mTLS session binding).

No persistent format, wire protocol or SQL grammar change. `dap-admin`
maps the new variant to its existing anyhow message pattern.
