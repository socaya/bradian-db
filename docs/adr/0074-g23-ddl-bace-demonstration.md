# ADR 0074 — G23 DDL coordinator BACE demonstration tests (implementation; **PROPOSED**)

**Status: PROPOSED — not accepted for implementation.**

Blocked on the accepted BACE workstream through ADR 0073. This
ADR closes the scope-limit deferrals explicitly recorded in
ADRs 0070 (§6 item 6), 0071 (§3.5 items 4–5), and 0072
(§3.5 items 4–5): the "fresh-open + idempotent-checkpoint rule"
meant none of those ADRs could demonstrate actual BRPG v2 pages
on disk through their engine's test path.

The G23 DDL coordinator (`HistorySqlEngine::execute_ddl` +
`enable_epoch_row_writes`) already works through a
BACE-wired engine — it's the natural mutation path that forces
a post-migration checkpoint. This ADR is **pure test
demonstration**: no new code, just honest coverage of what the
plumbing already does.

Owner review required before any code lands per CLAUDE.md §4.9.

## 1. Context

- ADRs 0070–0072 all deferred the same test shape: "legacy
  open on a BACE-written root fails closed, and v2 pages are
  actually produced through this engine." The deferral was
  honest: `migrated()` creates a G24 root checkpointed once
  at a fixed LSN; opening with `_with_bace` and immediately
  calling `backup_to` triggers `RowStore::checkpoint`'s
  idempotent-at-current-LSN rule and the existing v1
  snapshot is reused. No v2 pages get produced through the
  coordinator layer.
- The DDL coordinator (`HistorySqlEngine::execute_ddl`) is
  the natural mutation path: it advances both the catalog
  LSN and the history-domain LSN through its activation
  protocol (ADR 0033, ADR 0034, ADR 0040). A subsequent
  `checkpoint()` then emits v2 pages under BACE wiring per
  ADR 0065.
- Nothing about the DDL coordinator itself needs BACE
  changes. It already flows through the already-BACE-wired
  stores. What's missing is **test coverage** that
  exercises the full path and asserts the deferred
  invariants.
- This is the cleanest remaining "operator-facing verification"
  of the BACE workstream before moving into carrier work
  (`.derc`, BRWL) or new security primitives
  (authenticated envelope).

## 2. Scope of this ADR

**In scope:**

- New test file
  `crates/bradian-relational/tests/g23_ddl_bace_demonstration.rs`
  containing tests that:
  1. Open the migrated G24 fixture with
     `HistorySqlEngine::open_with_bace(Balanced + ZSTD)`.
  2. Call `enable_epoch_row_writes()` + a bounded
     `execute_ddl` statement (e.g.,
     `ALTER TABLE bank.public.accounts ADD memo TEXT`).
  3. Call `checkpoint()` to force snapshot emission.
  4. Assert the resulting snapshot files on disk contain v2
     pages (feature-flag bit set, envelope magic `BACE`,
     codec_id = ZSTD).
  5. Close the engine and assert
     `HistorySqlEngine::open` (legacy) on the same root
     fails closed with a BACE-origin error.
  6. Re-opening with `_with_bace` succeeds and reads the
     post-DDL state.
- Analogous demonstration for the backup side: a BACE G24
  backup produced AFTER a DDL mutation has v2 pages inside
  the sub-archive `.brp` files, and
  `restore_history_database_with_bace` round-trips through
  them.
- No code changes in `bradian-relational` or `dap-admin`.
  The ADR explicitly adds only tests.

**Explicitly out of scope** (each a separate future ADR):

- **G19 analog** (`CatalogLifecycle::open_with_bace` + DDL +
  v2-page demonstration). G19 doesn't have an `execute_ddl`
  surface with the same shape; its DDL happens through
  `SqlEngine`/`DomainSqlEngine` DDL + separate
  provisioning. Covered implicitly by ADR 0071 engine-level
  tests; not re-asserted here.
- **New error variants** anywhere.
- **Any change to the DDL coordinator's semantics,
  activation protocol, or lock discipline.**
- **Benchmarks of DDL-emitted v2 page size.**
- **Authenticated envelope, `.derc` BACE, BRWL BACE,
  dictionary scope** — each its own future ADR.

## 3. Proposed test design

### 3.1 Helper — `run_ddl_under_bace(root, wiring) -> ()`

```rust
fn force_v2_pages_via_ddl(root: &Path, wiring: &BaceWiring) {
    let mut db = HistorySqlEngine::open_with_bace(root, wiring.clone()).unwrap();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD memo TEXT").unwrap();
    db.checkpoint().unwrap();
    // engine drop releases locks
}
```

### 3.2 §3 test checklist

1. `ddl_under_bace_wiring_emits_v2_data_pages_in_history_domain`:
   after `force_v2_pages_via_ddl`, walk every
   `history-domain/snapshot-*.brp` page in `root`, find at
   least one with `format_version == 2` and envelope magic
   `BACE` at page offset 32.
2. `ddl_under_bace_wiring_emits_v2_data_pages_in_catalog`:
   same assertion on `catalog/snapshot-*.brp`. The catalog
   advances by 2 LSN during ALTER (intent + activation) so a
   checkpoint produces v2 pages here too.
3. `legacy_history_sql_engine_open_on_post_ddl_bace_root_fails_closed`:
   after `force_v2_pages_via_ddl`, calling
   `HistorySqlEngine::open(root)` returns an error whose
   string contains "open_with_bace" or "BACE" or
   "page_id".
4. `history_sql_engine_open_with_bace_after_ddl_reads_altered_schema`:
   reopen with `_with_bace`, SELECT the new column and
   verify it defaults to NULL.
5. `bace_wrapped_post_ddl_backup_round_trips_through_restore_with_bace`:
   produce a BACE backup via
   `HistorySqlEngine::backup_to_with_bace` AFTER the DDL,
   confirm the sub-archive `backup.info` flags are set AND
   the sub-archive `.brp` files contain v2 pages, then
   restore via `restore_history_database_with_bace` and
   reopen via `HistorySqlEngine::open_with_bace`.

### 3.3 Fixture reuse

The existing `tests/support/epoch_fixture.rs` already
provides `migrated()` which returns a provisioned G23
directory. This ADR uses it unchanged.

## 4. Alternatives considered

**A. Add a public `RowStore::force_checkpoint_v2()` helper
that writes an empty-but-v2 snapshot even at current LSN.**
Rejected: changes production code semantics for a test
convenience. The idempotent-checkpoint rule exists for a
reason.

**B. Mutate rows via `with_session` INSERT instead of DDL.**
Rejected: `with_session` row writes require epoch rows
enabled, which requires the DDL coordinator anyway. The
`execute_ddl` path is the most direct and idiomatic.

**C. Skip the demonstration and leave ADRs 0070–0072 scope
limits documented.** Rejected: the scope limits are
*observable gaps in test coverage*; closing them is valuable
and essentially free (one new test file, no production
changes).

**D. Fold the demonstration into the existing
`history_backup_consistency.rs` test file.** Rejected: that
file is about backup/restore invariants. A dedicated file
tells future readers "these tests exist specifically to
close the ADR 0070–0072 scope limits."

## 5. Open design questions (owner review)

1. **Test file name.** Draft:
   `g23_ddl_bace_demonstration.rs`. Confirm.
2. **Should the new test file include a short comment
   referencing ADRs 0070/0071/0072 scope-limit notes?**
   Draft: YES — explicit so a future reader understands
   why the file exists. Confirm.
3. **G19 analog tests.** Draft: out of scope. Confirm
   defer.

## 6. Impact on invariants

- **INV-001 commit correctness:** no change.
- **INV-002 one authoritative state:** no change.
- **INV-003 no authorization gap:** no change.
- **INV-004 no unsafe automatic availability:** N/A.
- **INV-005 recoverable lifecycle:** test-only; no
  production behavior change.

## 7. Dependencies this ADR would add

None. One new test file.

## 8. What this ADR explicitly does NOT do

- Touch any production source code.
- Add new APIs, error variants, policy keys, or codecs.
- Benchmark anything.
- Change the DDL coordinator.
- Address the four other remaining BACE candidates
  (authenticated envelope, `.derc`, BRWL, dictionary scope).

## 9. Acceptance evidence required before marking Accepted

1. ADRs 0060–0073 accepted — **satisfied**.
2. Owner sign-off on §5 open design questions.
3. Implementation satisfying every item in §3.2.
4. No regressions in existing test files.

Only then can this ADR move from **PROPOSED** to **ACCEPTED**.
