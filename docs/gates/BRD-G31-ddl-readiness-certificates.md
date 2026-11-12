# BRD-G31 — Recoverable inactive DDL readiness certificates

**Baseline:** Bradian Master Architecture & Software Requirements Specification v1.1  
**Implementation:** `crates/bradian-relational/src/durable_typed.rs`, `history_sql.rs`  
**Status:** Source and three regression test cases committed on `main`. Cargo execution, crash injection, and production qualification are outstanding.

## Scope and rationale

G11 catalog events and G22 durable historical row versions use independent WALs. Bradian must not claim transactional `ALTER TABLE` without a protocol that safely handles crashes between commits. G31 deliberately implements an **inactive readiness checkpoint** for a G28 proposal already bound to a G30 catalog LSN. It does not switch the active table definition, rewrite rows, or publish DDL to the catalog.

## Implemented

- [x] Reserved G22 user-key namespace `\\0bradian-ddl-readiness-v1:` + fixed 8-byte big-endian TableId.
- [x] Immutable JSON `DdlReadinessRecord` with strict deserialization: TableId, G30 coordination commit LSN, G28 proposal commit LSN, G11 catalog LSN, and proposal checksum.
- [x] `HistorySqlEngine::certify_schema_readiness(qualified_table)` verifies catalog ownership and G30 fence, then writes one synchronized WAL record.
- [x] Refusal for missing fence, wrong catalog LSN, already aborted proposal, or duplicate readiness insert.
- [x] On opening G22 historical storage, replay checks exact namespace/key length, complete record, matching G30 fence, strict commit ordering, and absence of an earlier abort.
- [x] G23 current and historical typed-row scans skip the readiness namespace.
- [x] `HistorySqlEngine::ddl_readiness_records()` returns the verified opening-snapshot list; reopen to refresh changes made after the owner was opened.
- [x] G24 archive/restore carries and validates readiness certificates in the existing historical WAL.
- [x] Three regression tests committed for restart persistence, invalid/cancelled refusal, and archive recovery.
- [x] No persisted G22 row-version format changes and no CI automation changes.

## Example

```rust
let db = HistorySqlEngine::open("./history-db")?;
// Requires a G28 proposal and a matching G30 coordination fence.
let readiness_lsn = db.certify_schema_readiness("bank.public.accounts")?;
```

**The schema remains inactive.** `SELECT`, `INSERT`, and `UPDATE` still use the original immutable G11 definition. A readiness record is an audit/recovery checkpoint, not an active schema epoch, acceptance test, or proof of operational approval.

## Known limitations and remaining activation work

- [ ] Catalog ALTER command and replay-validated schema revision events.
- [ ] Coordinated two-WAL publication, idempotent transition phases and deterministic crash recovery.
- [ ] Version-tagged row encoding and mixed-epoch historical decoding.
- [ ] Logical row migration, constraints and backfill/default semantics.
- [ ] Explicit post-readiness cancellation policy and retention/GC rules.
- [ ] Forced-crash, disk-full, malformed WAL, and multi-platform recovery drills.

A later G29 abort may follow an inactive readiness record: this does **not** activate the schema. If the abort occurs before readiness, readiness certification is refused. G30 catalog-horizon checks still apply to all coordination records.

## Required manual validation

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

These have **not been executed** in the present environment because Cargo and Rust are unavailable. Committed source and test cases do not imply passing compilation or production readiness.
