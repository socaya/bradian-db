# BRD-G32 — Explicit schema-epoch row envelope

**Baseline:** Bradian Master Architecture & SRS v1.1  
**Implementation:** `crates/bradian-relational/src/epoch_row.rs`  
**Status:** Opt-in implementation and three unit test cases committed directly to `main`; compilation and runtime execution not verified.

## Objective and actual delivery

The existing G4/G23 typed row format `BRRW` v1 identifies a table but is not bound to a historical schema epoch. G11's catalog currently cannot replay a durable ALTER revision, while G22-G31 are still restricted to immutable catalog pins and inactive schema proposals. G32 therefore introduces a **separate, explicitly opt-in physical row envelope**, not a premature live migration.

- [x] `EpochSchemas`: explicit registry keyed by `(TableId, epoch LSN)`; re-registering an epoch with a differing schema is rejected.
- [x] `BRVE` v1 envelope: magic (4 bytes), version (1), table ID (u64 LE), epoch (u64 LE), payload length (u32 LE), payload CRC32 (u32 LE), followed by the existing validated `BRRW` row bytes.
- [x] Strict parsing validates version, nonzero identities, declared size, inline capacity, CRC32 and known epoch. No decoding under the newest schema when an exact epoch is unavailable.
- [x] `encode(table_id, epoch, values)` encodes only rows valid under their registered schema.
- [x] `decode(bytes)` returns original TableId, epoch and typed values using the exact registered table layout.
- [x] `decode_legacy(table_id, epoch, bytes)` supports existing BRRW rows only when the caller explicitly supplies the already-authoritative original epoch; it never guesses from the physical data.
- [x] Enforces the G22 inline value limit (3545 bytes) for the entire envelope.
- [x] Three unit tests for mixed epoch roundtrips, legacy decoding, corruption/truncation/unknown epoch, rebinding refusal and invalid row values.
- [x] New module exported without modifications to persisted G11, G22 or G23 formats, SQL command behavior, backup archives or CI configuration.

## Rust usage

```rust
use bradian_relational::epoch_row::EpochSchemas;
use bradian_relational::typed_table::Value;

let mut schemas = EpochSchemas::default();
// Tables must be validated and epochs derived from a future coordinated
// authoritative schema journal, not arbitrary client input.
schemas.register(original_epoch, original_table)?;
schemas.register(next_epoch, next_table)?;
let encoded = schemas.encode(table_id, original_epoch, &[
    Value::Int64(1), Value::Int64(100),
])?;
let decoded = schemas.decode(&encoded)?;
assert_eq!(decoded.epoch, original_epoch);
```

## Explicit security and recovery boundaries

This codec is a building block for future multi-epoch historical reads. No G11 catalog ALTER command, coordinated catalog/history activation, schema epoch WAL publication, or row migration is implemented by G32. Existing G23 `SELECT AS OF` and G31 readiness APIs still use the original immutable table definition. `EpochSchemas` is an in-process registry, not a persistent catalog. Epoch IDs must eventually be supplied exclusively by replay-verified durable catalog/history coordination; arbitrary untrusted registrations are not sufficient authentication.

CRC32 detects accidental payload damage, not malicious modification. The inline envelope reduces available row payload capacity. Future integration must explicitly test retention, crash recovery, mixed-epoch PK indexes, checkpoint/backup, and garbage-collection constraints.

## Manual verification required

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

These commands were not executable in the present environment because the Rust toolchain was unavailable. Do not interpret committed tests as observed passing tests.

## Next engineering priority

G33 should implement replay-validated, **inactive** catalog schema-revision records with stable identities and compatibility checks; only then design an atomic/recoverable activation protocol binding G11, G22 and `BRVE` row epochs.
