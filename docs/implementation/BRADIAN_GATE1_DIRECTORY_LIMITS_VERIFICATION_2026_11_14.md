# Master Gate 1 — Directory listing admission verification

14 November 2026. Baseline: `b3e22f4886d51907611ed5b4f2aa52706216bf2b`.
Stable task G1-MEMORY in fixed Gates 0–9. Gate 1 remains active.

## Executed result

**779 Rust tests passed, 0 failed, 0 ignored**, including **504 relational
tests**. Workspace compilation and the directory-admission tests passed. CI
remains manual-only. No dependency, lockfile, persistent format, SQL grammar or
default isolation changed.

Environment: Darwin 25.2.0 x86_64, Rust/Cargo stable.
The checkout was restored from published `main` before implementation.

| Executed check | Result |
|---|---|
| `cargo check --workspace` | Passed |
| `cargo test --workspace` | 779 passed, 0 failed, 0 ignored |
| `cargo test -p bradian-relational --test directory_limits` | 5 passed |
| `cargo test -p bradian-relational --test legacy_migration` | 15 passed (2 new) |

Clippy, Studio typecheck/build and `scripts/verify_roadmap.py` were not executed
on this increment — follow-up documentation runs them before any release claim.

## Implemented and exercised

`DirectoryLimits { max_entries: 65_536, max_name_bytes: 4 MiB }` is a per-listing
admission policy added to `runtime_limits.rs`. `StorageLimits.directory` carries
the policy through `RowStore::open_with_policies`,
`RowStore::restore_backup_with_policies` and `RowStore::storage_limits()`. A new
`ResourceError::DirectoryLimit { resource, limit }` is distinct from corruption,
`std::io::Error`, cache and index/row-store resource errors.

`RowStore::list_snapshots` and `prune_snapshots` enumerate the row-store root
under this admission. The pre-manifest compatibility path (used when
`checkpoint.current` is absent) applies the policy without changing on-disk
state on refusal. `legacy_migration::source_table_ids` adopts the same helper;
a new `migrate_legacy_sql_with_policies` accepts an explicit
`DirectoryLimits`, and the default `migrate_legacy_sql` preserves its prior
signature with `DirectoryLimits::default()`.

Five new tests in `tests/directory_limits.rs` cover (1) entry-count refusal on
the pre-manifest reopen path, (2) name-byte refusal on the same path,
(3) default policy admitting an ordinary store, (4) on-disk preservation under
refusal (quota is not corruption), and (5) policy round-trip through backup and
restore. Two new tests in `tests/legacy_migration.rs` cover entry-count refusal
during migration (destination not published) and the default path still
accepting a normal legacy tree. Existing SQL, analytical, recovery, corruption,
backup/migration and admission suites still pass.

## Remaining scope

Short "is this directory empty" probes (`fs::read_dir(_).next().is_some()` in
`domain_sql`, `history_sql`, `history_migration`, `lifecycle`, `mvcc_sql`) read
at most one entry and are not admitted — adding accounting for a constant
amount of work would invent a measurement without strengthening the invariant.

Catalog, domain, lifecycle and user-data directories outside the two adopted
sites are not yet admitted. A global per-process directory/inode budget, an
open-file-descriptor cap and filesystem-scanning tools remain open. Metadata
JSON admission (ADR 0048), scan controls (ADR 0041), staging (ADR 0044),
backing maps (ADR 0045), index recovery (ADR 0046), checkpoint/build (ADR 0047),
CLI output (ADR 0049) and historical transaction admission (ADR 0050) are
unaffected. No cancellation, OOM, failed-I/O, physical power-loss, browser or
macOS/Windows qualification is claimed. Gate 1 remains active; see
[ADR 0051](../adr/0051-bounded-directory-listings.md) and the
[fixed roadmap](BRADIAN_MASTER_ROADMAP.md).
