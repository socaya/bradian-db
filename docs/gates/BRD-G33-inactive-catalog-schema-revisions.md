# BRD-G33 — Replay-validated inactive catalog schema revisions

**Baseline:** Bradian Master Architecture and SRS v1.1  
**Implementation:** `crates/bradian-relational/src/persistent_catalog.rs`  
**Status:** Source and two regression test cases committed to `main`; compilation, test execution, forced-crash recovery, and operational qualification outstanding.

## Implemented increment

G33 adds an **inactive, append-only proposed table revision event** in the existing G11 catalog journal. It does not rewrite the live catalog table, modify G22 historical schema pins, encode new BRVE rows, or activate SQL ALTER.

- [x] `ProposeTableRevision { table: TableId, proposed: Table }` added to the existing version-1 catalog WAL command vocabulary without changing the surrounding DDL-event representation.
- [x] Catalog owner stores proposals separately from authoritative live `Catalog` state in a `BTreeMap<TableId, Table>`.
- [x] During commit and replay, validate against the *original* live table through G27 `plan_nullable_additions` (identity, existing columns and constraints unchanged; only trailing nullable columns).
- [x] One immutable pending proposal per TableId. Duplicate or incompatible proposals fail without a new WAL event.
- [x] A table with a pending proposal cannot be dropped via the G11 catalog API.
- [x] Replay rebuilds the pending map from ordered, contiguous journal events and fails closed on contradictory or malformed entries.
- [x] `PersistentCatalog::propose_table_revision(TableId, Table)` appends one event. `proposed_table_revision` and `proposed_table_revisions` expose inactive revisions.
- [x] Existing G11 `snapshot()` retains its previous semantics: only live definitions are returned. G23 historical SQL continues using immutable original definitions.
- [x] Two committed integration tests cover checkpoint/restart, stable active schema, duplicate/drop refusal, and incompatible proposal rejection.
- [x] CI remains manual-only.

## Rust example

```rust
let catalog = PersistentCatalog::open("./catalog")?;
let original = catalog.snapshot().table(table_id).unwrap().clone();
let mut candidate = original.clone();
candidate.columns.push(Column {
    name: "description".into(),
    data_type: DataType::Text,
    nullable: true,
});
catalog.propose_table_revision(table_id, candidate)?;
assert_eq!(catalog.snapshot().table(table_id).unwrap(), &original);
```

## Coordination restrictions

**Do not use G33 on a live historical root already fenced by G30 without a future coordinated catalog/history protocol.** G30 records the exact G11 catalog LSN; a G33 catalog event advances that LSN. The existing G30 verifier therefore correctly refuses to reopen a history database with the now-divergent catalog. The separate G33 API intentionally makes no claim of multi-WAL atomicity, and it is not exposed through G23 HistorySqlEngine.

G33 does not yet bind its proposed catalog record to G28's matching history proposal or G31 readiness certificate. It cannot roll forward or back from a cross-store crash. The G32 BRVE row codec is still opt-in and not connected to SQL.

## Remaining work

- [ ] Coordinated catalog/history intent protocol and safe matching of proposals by stable IDs, schema digest and WAL epochs.
- [ ] Transactional activation of a validated revision under a proven crash-recovery state machine.
- [ ] Row-to-epoch binding and migration of existing BRRW records into live mixed-schema decoding.
- [ ] Catalog revision cancellation, garbage collection, and lifecycle rules for inactive proposals.
- [ ] Failpoint injection, disk-full and torn-WAL recovery tests, live build checks, and large-data validation.

## Manual verification

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

These commands have **not been executed successfully in this environment**. Source-level test cases do not prove passing builds or production readiness.
