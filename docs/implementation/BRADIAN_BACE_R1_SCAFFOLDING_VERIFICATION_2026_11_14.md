# BACE R.1 — `row_store` scaffolding verification

14 November 2026. Baseline: `43e8986`.
First of four ADR 0065 sub-slices. **No BRPG v2 bytes reach disk
yet** — this slice adds the entry point, the wiring field and the
new error variants without wiring the checkpoint writer or the
recovery reader. R.2 (next) lands the writer.

## Executed result

**918 Rust tests passed, 0 failed, 0 ignored.** Clippy clean on
`bradian-relational --all-targets`.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace` | 918 passed, 0 failed |
| `cargo test -p bradian-relational --test row_store_bace` | 4 passed |
| `cargo clippy -p bradian-relational --all-targets -- -D warnings` | Passed |

## What landed

- `BaceWiring { policy_service, registry }` struct in
  `row_store.rs`. `policy_service: Arc<dyn PolicyService + Send + Sync>`
  for live consultation; `registry: Arc<dyn CodecRegistry + Send + Sync>`
  for codec lookup.
- New `bace_wiring: Option<BaceWiring>` field on `RowStore`.
- New entry point `RowStore::open_with_bace(root, StorageLimits,
  BaceWiring) -> StoreResult<Self>`. Internally delegates to
  `open_with_policies` and then stores the wiring on the returned
  store. Existing `open` / `open_with_limits` /
  `open_with_recovery_limits` / `open_with_policies` all keep
  `bace_wiring = None`.
- Five new `StoreError` variants (per ADR 0065 §3.7):
  - `BaceCodecUnavailable { codec_id }`
  - `BaceCodecFormatVersionMismatch { codec_id, declared, build_supports }`
  - `BaceEnvelope(EnvelopeError)` (via `#[from]`)
  - `BaceDecode(DecodeError)` (via `#[from]`)
  - `BaceOpenWithoutBaceWiring { page_id }`

The wiring is **stored but not consulted yet**. The checkpoint
writer and the recovery reader continue to take the v1 path;
compilation succeeds because the new variants and field are
unused in-tree.

## 4 unit tests

- `open_with_bace_opens_fresh_directory_without_error` — basic
  smoke test.
- `open_with_bace_round_trips_plain_writes_without_touching_disk_format`
  — a write/checkpoint/reopen cycle through `open_with_bace`
  produces identical row visibility to the v1 path.
- `legacy_open_still_round_trips_against_an_open_with_bace_store`
  — because R.1 does not yet emit v2 pages, a store written
  through `open_with_bace` remains readable by the legacy `open`
  path. R.2 breaks this (deliberately) when the policy resolves
  to non-Raw.
- `bace_wiring_can_be_constructed_with_a_raw_profile_policy` —
  smoke test that the Slice Q types compose with the new
  `BaceWiring`.

## Compliance check against ADR 0065

- §3.1 entry point — ✓
- §3.6 admission interactions (no new `ResourceError`) — ✓
- §3.7 new `StoreError` variants — ✓ (all five present)
- §4 test items 1 (every existing test passes) and 12
  (`open_with_bace` on a legacy v1 directory works) — ✓
- §9 "what this does NOT do" — ✓ (writer and reader unchanged;
  no codec invoked; no envelope read or written)

## Next

- **R.2** — checkpoint writer branch. `write_snapshot` gains an
  `Option<&BaceWiring>` argument; non-Raw profile emits BACE-
  wrapped v2 pages via `Page::new_v2`.
- **R.3** — recovery reader branch. `read_latest_snapshot` and
  `read_indexed_record` learn to decode BACE envelopes.
- **R.4** — full 12-item test matrix from ADR 0065 §4.

See [ADR 0065](../adr/0065-row-store-bace-checkpoint-integration.md),
[BACE_ADR_REVIEW_PACKAGE](BACE_ADR_REVIEW_PACKAGE.md).
