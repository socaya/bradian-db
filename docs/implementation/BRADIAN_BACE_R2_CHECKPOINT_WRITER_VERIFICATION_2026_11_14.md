# BACE R.2 — checkpoint writer verification

14 November 2026. Baseline: `7f33993`.
Second of four ADR 0065 sub-slices. **First BRPG v2 bytes reach
disk under the Balanced profile.** Raw profile remains
byte-identical to v1. Recovery reader (R.3) is next; a store
checkpointed under Balanced in this slice is readable only by a
Balanced-aware reopen once R.3 lands — until then, reopen with
Balanced fails `invalid snapshot row length` because the recovery
path tries to parse envelope bytes as plain row records.

## Executed result

**921 Rust tests passed, 0 failed, 0 ignored.** Clippy clean on
`bradian-relational --all-targets`.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace` | 921 passed, 0 failed |
| `cargo test -p bradian-relational --test row_store_bace` | 7 passed |
| `cargo clippy -p bradian-relational --all-targets -- -D warnings` | Passed |

## What landed

`crates/bradian-relational/src/row_store.rs` grew three things:

1. `resolve_bace_codec(Option<&BaceWiring>) -> StoreResult<Option<(CodecId, &dyn Codec)>>`
   — resolves the policy at `ScopeChain::baseline_only()` (the
   `RowStore` layer has no DB/schema/table context). Returns `None`
   when wiring is absent or the profile is `Raw`; returns
   `Some((id, codec))` for any other profile, or fails closed with
   `BaceCodecUnavailable` when the allow-set contains only
   unregistered IDs.
2. `emit_data_page(file, page_id, lsn, plain, Option<(CodecId, &dyn Codec)>) -> StoreResult<()>`
   — single helper that writes either a v1 data page (no BACE) or a
   v2 data page (plain bytes → codec → envelope → `Page::new_v2`
   with `payload_is_bace_envelope`).
3. `write_snapshot` signature extended with `bace: Option<&BaceWiring>`.
   Under BACE, per-page plain capacity shrinks from `MAX_PAYLOAD_SIZE`
   to `MAX_PAYLOAD_SIZE - ENVELOPE_HEADER_SIZE - ENVELOPE_TRAILER_SIZE`
   (= 4028 bytes) so the envelope fits in a 4 KiB page. Metadata
   page (page 0) stays v1 per ADR 0065 §6 item 7.

The `checkpoint_with_limits` call site passes
`self.bace_wiring.as_ref()` through to `write_snapshot`.

### Pointer semantics

`RowPointer` offsets reference the **decoded** (plain) payload
position. Both the v1 and v2 writers compute offsets on the plain
buffer; under BACE, the recovery reader will decode the envelope
first and apply the offset to the decoded bytes (R.3). The index
build (`DiskBTree::build_with_limits`) consumes the pointers
unchanged.

### Strict checkpoint failure

Any BACE error in the write path aborts the whole checkpoint. The
`.tmp` is cleaned, the manifest is not updated, and the WAL is not
truncated. ADR 0065 §3.3 fail-closed invariant.

## 3 new tests (R.2)

Added to `tests/row_store_bace.rs` on top of the four R.1 tests:

- `raw_profile_emits_v1_bit_identical_to_legacy_open` — ADR 0065
  §4 test item 3. A store opened with `CompressionProfile::Raw`
  produces a snapshot file **byte-identical** to the legacy-open
  path on the same rows. Proves the Raw-opt-out path costs zero
  bytes on disk.
- `balanced_profile_emits_v2_data_pages_with_bace_envelope_flag_set`
  — ADR 0065 §4 test item 4. A store opened with
  `CompressionProfile::Balanced` emits a v2 data page whose
  `format_version == 2`, feature-flag bit 0 set, and whose payload
  at offset 32 starts with `"BACE"`. Metadata page 0 stays v1.
- `checkpoint_fails_closed_when_allowed_codec_set_contains_only_unregistered_ids`
  — ADR 0065 §4 test item 9. A policy whose allow-set contains
  only `CodecId(16)` (unassigned per ADR 0061 P1) and whose
  baseline is set to the same value (no capability-veto fallback
  escape) fails checkpoint with
  `BaceCodecUnavailable { codec_id: CodecId(16) }`. No snapshot
  file is published; no manifest update.

Plus two R.1 tests updated to force `CompressionProfile::Raw`
explicitly (they previously used the default `Balanced` profile
which R.2 now emits as v2 — and recovery can't read v2 yet).

## Compliance against ADR 0065

- §3.2 checkpoint writer branch — ✓ (`emit_data_page` +
  `resolve_bace_codec`).
- §3.3 strict checkpoint failure — ✓ (any BACE error aborts).
- §3.6 admission interactions (no new `ResourceError`) — ✓.
- §4 test items 1, 3, 4, 9 — ✓.
- §4 test items 2, 5, 6, 7, 8, 10, 11, 12 — **deferred to R.3 and
  R.4**: test 2 (v1 bit-compatibility of legacy `open`) is covered
  implicitly; tests 5 (round-trip), 7 (cap enforcement), 10
  (format version mismatch) need the recovery reader; test 8
  (old open on v2 fails closed) needs R.4.
- §9 "what this does NOT do" — ✓: no change to btree.rs,
  buffer_pool.rs, backup.rs, migrations, WAL; no new codec; no
  avoid-expansion selection; no SQL surface; no policy
  persistence.

## What this component explicitly does NOT do

- Recovery reader is unchanged — `read_latest_snapshot` and
  `read_indexed_record` still treat every page payload as plain
  bytes. A store checkpointed under Balanced in R.2 is readable
  only once R.3 lands.
- No `BaceOpenWithoutBaceWiring` emission yet — legacy `open` on
  a v2 directory fails inside `read_latest_snapshot` with the
  pre-existing `Corruption("invalid snapshot row length")`, which
  R.3 upgrades to the specific `BaceOpenWithoutBaceWiring` error.
- `read_indexed_record` is unchanged. Indexed reads from a v2
  checkpoint will fail with corruption errors until R.3 wires the
  decode path.
- No new test for the Balanced-profile round-trip (writer-only in
  this slice). R.3 adds it.

## Next

R.3 — `read_latest_snapshot` and `read_indexed_record` decode BACE
envelopes before applying row offsets. The new error variants from
R.1 (`BaceOpenWithoutBaceWiring`, `BaceEnvelope`, `BaceDecode`,
`BaceCodecFormatVersionMismatch`) get their first fires.

See [ADR 0065](../adr/0065-row-store-bace-checkpoint-integration.md).
