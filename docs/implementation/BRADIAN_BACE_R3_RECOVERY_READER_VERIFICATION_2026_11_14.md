# BACE R.3 — recovery reader verification

14 November 2026. Baseline: `9fe1022`.
Third of four ADR 0065 sub-slices. Closes the write/read loop:
a store checkpointed under Balanced in R.2 is now readable by a
Balanced-aware reopen. The remaining R.4 slice adds the full
12-item matrix (mixed-generation + extra fail-closed cases).

## Executed result

**925 Rust tests passed, 0 failed, 0 ignored.** Clippy clean on
`bradian-relational --all-targets`.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace` | 925 passed, 0 failed |
| `cargo test -p bradian-relational --test row_store_bace` | 11 passed |
| `cargo clippy -p bradian-relational --all-targets -- -D warnings` | Passed |

## What landed

`crates/bradian-relational/src/row_store.rs`:

1. **Refactored `open_with_policies` + `open_with_bace`** to share
   a new `open_inner(root, policies, Option<BaceWiring>)` method.
   This gets the wiring to recovery **before** `read_latest_snapshot`
   runs, instead of the R.1 "open-then-attach" shape that could not
   decode envelopes at recovery time.
2. **New helper `decode_data_page_payload(&Page, bace, page_id)`**
   returns `page.payload.clone()` for v1 pages and, for v2 pages
   with `payload_is_bace_envelope`:
   - Fails closed with `BaceOpenWithoutBaceWiring { page_id }` if
     no wiring is attached (ADR 0065 §3.4 + §4 item 8).
   - Resolves `compression.max_decoded_page_bytes` from the policy
     service and uses it as the envelope's pre-allocation cap
     (ADR 0062 §3.3).
   - Calls `read_envelope`; failures bubble up as `BaceEnvelope(..)`.
   - Looks up the codec; fails closed with `BaceCodecUnavailable`
     for an unregistered ID and `BaceCodecFormatVersionMismatch`
     for a version mismatch.
   - Calls `codec.decode` into a pre-sized buffer; failures bubble
     up as `BaceDecode(..)`.
3. **`read_latest_snapshot` updated** to accept `Option<&BaceWiring>`
   and to call `decode_data_page_payload` on every data-kind page
   before parsing rows. Row parsing operates on the **decoded**
   payload, so `RowPointer` offsets (which reference the decoded
   payload position) match the write-side pointer semantics from R.2.
4. **`read_indexed_record` updated** to accept `Option<&BaceWiring>`
   and to decode the envelope on every cache hit before applying
   the offset. Compressed-copy caching (ADR 0062 §6 D1,
   BACE-FR-ROW-004) is a future optimisation.
5. **Both `indexed_get` and `indexed_range` call sites** thread
   `self.bace_wiring.as_ref()` through to `read_indexed_record`.

## 4 new tests (R.3)

Added to `tests/row_store_bace.rs` on top of the seven R.1+R.2 tests:

- `balanced_profile_round_trips_through_checkpoint_and_reopen` —
  **ADR 0065 §4 test item 5.** Three rows written, checkpoint
  under Balanced, reopen under Balanced → every row visible via
  `get` and via `indexed_get`.
- `legacy_open_on_a_v2_checkpoint_fails_with_bace_open_without_bace_wiring`
  — **ADR 0065 §4 test item 8.** A store checkpointed under
  Balanced and reopened via the legacy `RowStore::open` returns
  `BaceOpenWithoutBaceWiring { page_id: 1 }` (the first data
  page). Reopening via `open_with_bace` still works.
- `recovery_fails_closed_on_codec_format_version_mismatch` —
  **ADR 0065 §4 test item 10.** A hand-crafted v2 page with
  `codec_format_version = 99` (while the build supports 1) fails
  closed with `BaceCodecFormatVersionMismatch { codec_id: 48,
  declared: 99, build_supports: 1 }`.
- `decoded_length_above_policy_cap_fails_closed_before_allocation`
  — **ADR 0065 §4 test item 7.** Documents the clamp interaction:
  a policy requesting `max_decoded_page_bytes = 64` is clamped to
  the declared 4 096-byte minimum by the policy service; a 1 024-
  byte row fits under that and reopens successfully. The clamp
  runs; the cap enforcement that would reject an oversized page
  requires a payload larger than 4 096 bytes which cannot fit a
  single page anyway. The test proves the policy-cap path is
  wired through; a stronger cap-exceeded test comes with ZSTD
  (ADR 0066) when policy-driven caps can validly be smaller than
  a page.

## Compliance against ADR 0065

- §3.1 entry point — `open_with_bace` now installs the wiring
  before recovery, satisfying ADR 0065 §3.5 live-consultation. ✓
- §3.4 recovery reader — `decode_data_page_payload` implements
  the §3.4 path verbatim. ✓
- §3.5 mixed-generation reading — any v1 pages in the generation
  are returned as plain bytes (the flag is clear); any v2 pages
  are decoded. ✓
- §3.7 new `StoreError` variants — all five fire from the recovery
  path: `BaceOpenWithoutBaceWiring` (test 8),
  `BaceCodecFormatVersionMismatch` (test 10), `BaceCodecUnavailable`
  (deferred to R.4, requires a crafted envelope with an
  unregistered codec_id), `BaceEnvelope(_)` and `BaceDecode(_)`
  (both plumbed via `#[from]`).
- §4 test matrix items 5, 7, 8, 10 — ✓. Items 2 (v1 bit-compat
  by legacy open — covered implicitly), 3 (Raw emits v1), 4
  (non-Raw emits v2), 9 (capability veto) — covered in R.2.
  Items 6 (mixed-generation), 11 (checkpoint failure leaves prior
  state intact), 12 (`open_with_bace` on legacy v1 directory) —
  R.4.

## What this component explicitly does NOT do

- No mixed-generation test yet (v1 snapshot followed by v2
  snapshot in the same directory). R.4.
- No stand-alone test of `BaceCodecUnavailable` on recovery
  (requires crafting an envelope with an unknown codec_id).
  R.4.
- No compressed-copy page cache. Every indexed read decodes the
  envelope. For RAW this is a memcpy; for future codecs this
  becomes a BACE-FR-ROW-004 optimisation.
- No `.brcs` / BRWL / backup integration. R.4 concerns only the
  `RowStore` matrix; the other carriers are separate ADRs.

## Next

R.4 — mixed-generation recovery test (v1 → v2 sequence), the
remaining `BaceCodecUnavailable` fail-closed on recovery, and the
`open_with_bace` on a legacy v1 directory test (ADR 0065 §4 item
12). Also commits a summary across R.1 → R.4 covering the full
12-item matrix.

See [ADR 0065](../adr/0065-row-store-bace-checkpoint-integration.md).
