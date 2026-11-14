# Directory listing admission

This Master Gate 1 / G1-MEMORY policy bounds how much memory can be
consumed by iterating a storage directory. Two call sites enforce it
today: `RowStore::list_snapshots` (pre-manifest reopen path used when
`checkpoint.current` is absent) and `legacy_migration::source_table_ids`
(G17 enumeration of legacy per-table directories). Short "is this
directory empty" probes elsewhere read at most one entry and are not
admitted.

| Budget | Default | Accounted state |
|---|---|---|
| Entries per listing | 65_536 | Every `DirEntry` yielded by the iterator, including incidental files |
| Accumulated name bytes | 4 MiB | `OsString::as_encoded_bytes().len()` summed over admitted entries |

Admission runs entry-by-entry during the fallible `read_dir` iterator.
The entry counter increments first; the name-byte counter adds the
encoded `OsString` length. Exceeding either returns
`ResourceError::DirectoryLimit { resource, limit }` and the iterator
stops immediately. Previously admitted entries and partial results are
discarded.

## Native configuration and accounting

```rust
use bradian_relational::row_store::RowStore;
use bradian_relational::runtime_limits::{DirectoryLimits, StorageLimits};

let policy = StorageLimits {
    directory: DirectoryLimits { max_entries: 1024, max_name_bytes: 128 * 1024 },
    ..StorageLimits::default()
};
let db = RowStore::open_with_policies("./private-store", policy)?;
assert_eq!(db.storage_limits().directory.max_entries, 1024);
```

`RowStore::storage_limits()` reports the selected policy including the
directory budget. `RowStore::open` uses defaults. The policy flows into
`RowStore::restore_backup_with_policies` so backup validation refuses an
archive whose payload directory exceeds the caller's budget.

For `legacy_migration`, use `migrate_legacy_sql_with_policies`:

```rust
use bradian_relational::legacy_migration::migrate_legacy_sql_with_policies;
use bradian_relational::runtime_limits::DirectoryLimits;

let limits = DirectoryLimits { max_entries: 128, ..DirectoryLimits::default() };
let report = migrate_legacy_sql_with_policies(&source, &destination, limits)?;
```

The existing `migrate_legacy_sql` keeps its prior signature and uses
`DirectoryLimits::default()`.

## Refusal and recovery

A refusal is distinct from corruption, `std::io::Error` and
missing-directory handling. It leaves the filesystem exactly as it was:
no file is created, renamed or deleted. A migration refusal leaves the
destination unpublished; a later retry with a relaxed policy succeeds
without re-copying the source.

The `.next().is_some()` single-entry probes in `domain_sql`,
`history_sql`, `history_migration`, `lifecycle` and `mvcc_sql` are not
admitted; they read at most one entry and are O(1). Admission accounting
for a constant amount of work would invent a measurement without
strengthening the invariant.

## Example tightening

Operators enforcing a per-tenant filesystem budget can set a small
`max_entries` to refuse a shared store that has accumulated more files
than the tenant policy allows:

```rust
let strict = DirectoryLimits { max_entries: 256, max_name_bytes: 64 * 1024 };
// Open under the strict policy. If the store directory contains more
// than 256 published or incidental files, open returns DirectoryLimit.
let db = RowStore::open_with_policies(root, StorageLimits {
    directory: strict, ..StorageLimits::default()
});
```

Incidental files count toward the budget. If operators drop `.DS_Store`,
`README`, ad-hoc backups or editor temp files into the store directory,
they consume entries. Keep the store directory clean or raise the budget.

## Remaining scope

- Catalog, domain, lifecycle and user-data directories outside the two
  adopted sites are not yet admitted.
- A global per-process directory/inode budget, an open-file-descriptor
  cap and filesystem-scanning tools remain open — §13 owner-decision
  items in the
  [comprehensive implementation plan](../implementation/BRADIAN_COMPREHENSIVE_IMPLEMENTATION_PLAN.md).

See [ADR 0051](../adr/0051-bounded-directory-listings.md) and
[executed verification](../implementation/BRADIAN_GATE1_DIRECTORY_LIMITS_VERIFICATION_2026_11_14.md).
