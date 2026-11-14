# ADR 0053 — Private-root local access policy (G1-LOCAL-ACCESS)

**Status:** Accepted for the POSIX directory-mode invariants enforced by the
shared helper and used by the native SQL CLI. Gate 1 remains active. This
does not accept the full master G1-LOCAL-ACCESS exit (owner-UID enforcement,
POSIX ACL inspection, parent-path trust, Windows ACL and enterprise identity).

Local-access enforcement had grown inside `dap-admin/src/local_sql.rs::private_root`,
making it unreachable from library callers. Direct consumers of
`bradian_relational::row_store::RowStore::open` could not invoke the same
invariants without re-implementing them. This created a second, implicit
local-access contract with no central test surface.

A shared `bradian_relational::local_access` module now owns the invariants:

```rust
pub enum PrivateRootPolicy { RequireExisting, CreateIfMissing }
pub enum LocalAccessError {
    Unsupported,                 // non-Unix platform; CLI still bails.
    NotFound,                    // RequireExisting + absent root.
    NotADirectory,               // root exists but is not a directory.
    SymlinkedRoot,               // root is itself a symlink.
    LooseMode { mode: u32 },     // group or other bits are set.
    Io(std::io::Error),
}
pub fn ensure_private_root(path: &Path, policy: PrivateRootPolicy)
    -> Result<(), LocalAccessError>;
```

Enforced invariants (POSIX):

1. The root is not a symlink (`symlink_metadata` + `file_type().is_symlink()`
   refuses before any content check).
2. The root is a directory, not a regular file, device, FIFO or socket.
3. The file mode has no group or other permissions (`mode & 0o077 == 0`).
4. If missing and policy is `CreateIfMissing`, the directory is created with
   mode `0o700` via `DirBuilder::mode(0o700)`. If missing and policy is
   `RequireExisting`, `NotFound` is returned without creating anything.

On non-Unix platforms the helper returns `Unsupported`; the native CLI retains
its existing `bail!` so no platform silently skips enforcement.

`dap-admin`'s `private_root` delegates to the helper so there is now exactly
one implementation of the invariants; CLI behavior is byte-identical (the CLI
maps `LocalAccessError` to the same `anyhow::ensure!` messages it used before,
so error strings remain compatible with existing tests).

Not enforced by this ADR and explicitly open:

- Effective-UID ownership of the root and every ancestor. Requires `chown` or
  an injected ambient identity to test deterministically.
- POSIX ACL (`setfacl`) and extended attributes. A `0o700` directory can still
  carry an ACL granting access to other principals.
- Parent-path trust (world-writable or symlinked ancestors enabling TOCTOU
  rename). Needs a design that chooses between canonicalization and strict
  per-component checks.
- Windows file-ACL / SID enforcement; the shared helper returns `Unsupported`.
- Enterprise identity integration: SSO/mTLS session binding to the local
  operator; see the Bradian threat model for the full open set.

No persistent format, wire protocol or SQL grammar changes. No new dependency
is introduced.
