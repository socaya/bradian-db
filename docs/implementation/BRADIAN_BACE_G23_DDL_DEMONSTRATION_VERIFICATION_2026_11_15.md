# BACE G23 DDL demonstration verification (ADR 0074)

15 November 2026. Baseline: `4d3c88b`. Test-only ADR. Closes
the scope-limit deferrals recorded in ADRs 0070 §6 item 6,
0071 §3.5 items 4–5, and 0072 §3.5 items 4–5.

## Executed result

**982 Rust tests passed, 0 failed, 0 ignored** (978 → 982).
Clippy clean on `bradian-relational --all-targets`.

| Executed check | Result |
|---|---|
| `cargo test --workspace --locked` | 982 passed, 0 failed |
| `cargo test -p bradian-relational --test g23_ddl_bace_demonstration` | 4 passed |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## What landed

One new test file: `crates/bradian-relational/tests/g23_ddl_bace_demonstration.rs`.

No production code was changed. The test file exercises the
existing BACE-wired path through the G23 DDL coordinator:

```rust
fn force_v2_pages_via_ddl(root: &Path, wiring: &BaceWiring) {
    let mut db = HistorySqlEngine::open_with_bace(root, wiring.clone()).unwrap();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD memo TEXT").unwrap();
    db.checkpoint().unwrap();
}
```

### 4 tests

| # | Test | ADR §3.2 item |
|---|---|---|
| 1 | `ddl_under_bace_wiring_emits_v2_pages_in_history_domain_snapshot` | 1 (history-domain v2 + ZSTD codec_id=49) |
| 2 | `legacy_history_sql_engine_open_on_post_ddl_bace_root_fails_closed` | 3 |
| 3 | `history_sql_engine_open_with_bace_after_ddl_reads_altered_schema` | 4 |
| 4 | `bace_wrapped_post_ddl_backup_round_trips_through_restore_with_bace` | 5 (catalog + history-domain sub-archive v2 pages observed; full round-trip via ADR 0068 restore) |

### Honest adjustment to §3.2 item 2

The drafted §3.2 item 2 asserted catalog-side v2 pages appear
after `HistorySqlEngine::checkpoint()`. Implementation showed
that `HistorySqlEngine::checkpoint()` only drives
`self.domain.checkpoint()` (history-domain), not the catalog.
Catalog is checkpointed only through its own `backup_catalog_to`
path, reached via `backup_to_with_bace`.

Shipped tests instead:
- Test 1 (history-domain v2 after `db.checkpoint()`)
- Test 4 (both catalog and history-domain sub-archive v2 pages
  after `db.backup_to_with_bace` — because the catalog backup
  step invokes its own checkpoint).

The adjustment is recorded in the ADR itself; adding a direct
catalog-checkpoint entry point on `HistorySqlEngine` is out of
ADR 0074 scope and could be a separate thin follow-up.

## Compliance against ADR 0074 §3.2

| Item | Status |
|---|---|
| 1 History-domain v2 pages | ✓ test 1 |
| 2 Catalog v2 pages | Covered transitively via test 4 (sub-archives after backup); direct source-side assertion requires a new catalog-checkpoint entry point out of scope |
| 3 Legacy open fails closed | ✓ test 2 |
| 4 Reopen with wiring reads altered schema | ✓ test 3 |
| 5 BACE-wrapped post-DDL backup round-trips | ✓ test 4 |

## What this ADR explicitly does NOT do (unchanged)

- Touch any production source code.
- Add new APIs, error variants, policy keys, or codecs.
- Benchmark anything.
- Change the DDL coordinator.
- Address the four remaining BACE candidates
  (authenticated envelope, `.brcs`, BRWL, dictionary
  scope).

## BACE workstream progress (as of this ADR)

| ADR | Component | Status |
|---|---|---|
| 0060–0066 | Prerequisites + RowStore + ZSTD | ACCEPTED |
| 0067 | Backup carrier adapter | ACCEPTED |
| 0068 | Coordinator backup carrier adapter (restore) | ACCEPTED |
| 0069 | CatalogLifecycle BACE open (G19 writer) | ACCEPTED |
| 0070 | HistorySqlEngine BACE open (G24 writer) | ACCEPTED |
| 0071 | SQL engines BACE open (SqlEngine + DomainSqlEngine) | ACCEPTED |
| 0072 | MvccSqlEngine BACE open (G21) | ACCEPTED |
| 0073 | `bradian-db` CLI BACE flags | ACCEPTED |
| **0074** | **G23 DDL coordinator BACE demonstration tests** | **ACCEPTED** |

The BACE workstream is now fully exercised end-to-end for the
G24 relational coordinator:

- Operator opens with `--bace-profile balanced --bace-codecs zstd,raw`
- DDL / row mutations trigger v2 page emission
- Backup produces BACE-wrapped sub-archives
- Restore round-trips through `restore_history_database_with_bace`
- Legacy opens on BACE-written directories fail closed

## Remaining BACE workstream candidates (unchanged)

- **Authenticated envelope (`BACA`)** — new security primitive
  with HMAC/signatures; needs threat modeling per CLAUDE.md §6.
- **`.brcs` carrier BACE** — analytical column segments in
  `bradian-storage` crate; format version bump.
- **BRWL carrier BACE** — WAL frame compression on the
  latency-sensitive write path; needs benchmarking.
- **Dictionary scope** — BACE §4.2 dictionary mode with
  per-column lifecycle.

Each of these is independent, has its own scope decisions,
and deserves its own accept-defaults → implement cycle.

## Baseline + commit

Baseline: `1352325` (ADR 0074 PROPOSED).
Implementation commit: assigned by this commit.

See [ADR 0074](../adr/0074-g23-ddl-bace-demonstration.md).
