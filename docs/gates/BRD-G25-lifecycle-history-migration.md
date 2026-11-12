# BRD-G25 — G18 lifecycle-to-history verified cutover

**Baseline:** Bradian Master Architecture and SRS v1.1  
**Implementation:** `crates/bradian-relational/src/lifecycle_history_migration.rs`  
**Source regression tests:** `crates/bradian-relational/tests/history_sql.rs`  
**Status:** Code and test cases committed to `main`; Rust compilation, test execution and failure-injection qualification outstanding.

## Bounded delivered slice

G18 databases maintain a G11 catalog, G15 shared-WAL row domain and G18 lifecycle WAL journal. G23 historical databases instead use an immutable G11 catalog and the independent G22 version-key history domain. They must never silently share a physical root.

`migrate_lifecycle_to_history(source, destination)` is an explicitly requested **offline logical snapshot cutover**, implemented as:

1. Require an existing G18 catalog, shared-domain and lifecycle journal; refuse existing G23 history, missing components and overlapping destination paths.
2. Open `CatalogLifecycle`, reconcile pending provisioning intents, and capture a G19 three-component verified archive under an absent private sibling directory.
3. Release the G18 owner after the G19 snapshot; G18 source remains available to its operator but subsequent changes will **not** appear in the destination.
4. Restore the G19 archive into a second private sibling root. G19 reopens the complete lifecycle and verifies catalog, shared-domain schema pins, lifecycle identities, WAL LSNs and row counts.
5. Reopen the restored G18 copy and compare its catalog, shared-domain and lifecycle LSN/row-count identities against the captured G19 report **before** removing the legacy lifecycle journal. Only the restored temporary copy is stripped; the original three-component G18 source is never stripped.
6. Reuse G23 `migrate_shared_to_history` to import immutable schema pins and current typed rows in bounded G22 WAL batches; verify row contents and table IDs before publishing a separate, absent history root.
7. On successful independently verified G23 publication, clean temporary sibling archive/copy directories. Interrupted or failed conversion artifacts remain for operator inspection and block unsafe repeated attempts.

There is no attempted reconstruction of pre-cutover history: G18 only held latest committed row state. The first historical row version becomes visible when its G22 import batch commits.

## New public API

```rust
use bradian_relational::lifecycle_history_migration::migrate_lifecycle_to_history;

let report = migrate_lifecycle_to_history(
    "./g18-database",
    "./g25-history-database",
)?;
println!("Imported {} rows", report.imported.rows);
```

G23 `HistorySqlEngine::open` can then open the new destination. G24 backup/restore remains available for that new historical layout.

## Source-level regression cases

- [x] Verified G18 -> G23 copy retains all three source tables/components and typed row values.
- [x] Refuse locked G18 owners, and do not overwrite existing history destinations.
- [x] Refuse a root lacking G18 lifecycle metadata, left-over staging, and nested destination paths.
- [x] Keep CI manual-only and preserve old storage formats.

These cases are committed source, **not observed passing test results**.

## Required validation

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Qualification also requires filesystem power-loss tests during G19 capture, restore and G23 publication; disk-full tests; source-lock contention; import throughput; large dataset streaming; malicious symlink/rename races; physical flush semantics and manual restoration drills.

## Explicit limitations

- The capture is a point-in-time G19 snapshot; writes that occur **after** G19 capture do not automatically replicate to the new G23 database. Operators must coordinate the final service cutover.
- The G18 source is preserved; the routine does not change application routing, disable old writers or provide zero-downtime migration.
- Large source row snapshots and G23 retained history validation remain memory-bound.
- No mutable historical catalog, ALTER/DROP epoch support, online schema evolution, multi-node recovery, or serializable SQL.
- No cryptographic archive authentication, encryption or independent operational qualification.

**G25 delivers a guarded lifecycle-aware historical copy, not general historical schema evolution or a production-grade online migration.**
