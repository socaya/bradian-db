# ADR 0109 — Local-access parent-path trust (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Closes the
`parent-path trust` partial in the G1-LOCAL-ACCESS row of
the master roadmap.

## 1. Context

`bradian_relational::local_access` (ADR 0053, amended by
ADR 0057) enforces POSIX private-root invariants on the
data root itself: symlink refusal, mode `0700`, effective-UID
ownership. The gap register explicitly lists
**parent-path trust** as unqualified: if any ANCESTOR of the
data root is world-writable without the sticky bit (or is a
symlink), an attacker with access to that ancestor can
rename / replace the private root before the opening process
verifies it.

## 2. Scope

- `ParentPathPolicy::{AllowSticky, Strict, Ignore}` —
  default `AllowSticky` because `/tmp` (01777) is the Unix
  convention for safe world-writable directories.
- `check_parent_path_trust(path, policy)` — walks ancestors
  from `path.parent()` upward to the filesystem root. Rejects
  symlinks, world-writable-without-sticky dirs, and (under
  `Strict`) any world-writable ancestor.
- `ensure_private_root_with_parent_trust(path, root_policy,
  parent_policy)` — convenience combinator that runs the
  existing private-root check first, then the ancestor walk.
- 6 new tests in `src/local_access.rs::tests`.

## 3. Out of scope

- **Windows ACL** — still unqualified. ADR 0105 mentions
  `LockFileEx` semantics; parent-path analogue requires
  Windows SID / ACL inspection, which is a separate ADR.
- **POSIX ACL entries (`setfacl`)** — the standard mode bits
  are checked, but extended ACLs (`getfacl`) may still grant
  world-equivalent access. Future ADR.
- **Enterprise identity** — INV-003 remains open (ADR 0098).
- **Automatic wiring into RowStore::open** — this ADR ships
  the primitive and adds it to the `local_access` entry
  points; wiring the CLI / embedded callers to call the
  new function is a follow-up slice.

## 4. Compliance

1. `Ignore` policy skips the ancestor walk.
2. `AllowSticky` accepts `/tmp`-rooted temp dirs (01777).
3. `Strict` rejects sticky-world-writable ancestors with
   `UntrustedAncestor { path, mode: 0o777 }`.
4. `AllowSticky` rejects world-writable WITHOUT sticky (0o777).
5. `ensure_private_root_with_parent_trust` runs the root
   check first; loose root → `LooseMode` even under `Strict`
   parent policy.
6. Symlink ancestors always rejected (any policy ≠ Ignore).
7. 16 unit tests pass (10 pre-ADR-0109 + 6 new).
