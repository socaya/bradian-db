# ADR 0051 — Bounded directory listings (G1-MEMORY)

**Status:** Accepted for row-store snapshot discovery and legacy-migration table
enumeration. Gate 1 remains active.

Directory iteration previously materialized every `DirEntry` and derived name into
an in-process `Vec`/`BTreeSet` without an admission step. The number of files in a
row-store root and the number of legacy table directories are inputs from the
filesystem, so a hostile or broken source could grow these buffers without bound
before any content was read.

`DirectoryLimits { max_entries, max_name_bytes }` adds a conservative per-listing
budget. `StorageLimits.directory` carries the policy through row-store open,
backup and restore. Defaults admit 65_536 entries and 4 MiB of accumulated
`OsString` name bytes per listing. These are not total directory, filesystem,
inode or process memory bounds.

Admission runs entry-by-entry during the fallible `read_dir` iterator. Each
entry contributes one to the entry counter and the encoded length of its
`file_name()` to the name-byte counter. Exceeding either returns
`ResourceError::DirectoryLimit`. The iterator stops immediately; previously
admitted entries and partial results are discarded without touching persisted
state. Admission errors are distinct from corruption, `std::io::Error` and
missing-directory handling.

Two call sites adopt the admission helper now:

1. `RowStore::list_snapshots` (pre-manifest compatibility path during open and
   recovery). The policy comes from `StorageLimits.directory` and is also
   consulted by `RowStore::restore_backup_with_policies`.
2. `legacy_migration::source_table_ids`. A new
   `migrate_legacy_sql_with_policies` accepts an explicit `DirectoryLimits`; the
   default `migrate_legacy_sql` keeps the prior public signature and uses
   `DirectoryLimits::default()`.

Short "is this directory empty" probes (`fs::read_dir(_).next().is_some()` in
`domain_sql`, `history_sql`, `history_migration`, `lifecycle`, `mvcc_sql`) read
at most one entry and are left unchanged — adding an admission check there would
invent accounting for a constant amount of work.

Catalog/domain/lifecycle directories and user-data directories outside these two
sites remain unaccounted; global directory/inode budgets, open-descriptor caps
and filesystem-scanning tools remain open. Metadata JSON admission (`ADR 0048`),
scan controls (`ADR 0041`), staging admission (`ADR 0044`), backing-map
admission (`ADR 0045`), index-recovery admission (`ADR 0046`), checkpoint/build
admission (`ADR 0047`), CLI output admission (`ADR 0049`) and historical
transaction admission (`ADR 0050`) are unaffected. No persisted format, wire
protocol, SQL grammar or recovery semantic changes.
