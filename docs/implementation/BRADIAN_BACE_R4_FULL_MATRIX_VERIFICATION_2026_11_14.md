# BACE R.4 — full ADR 0065 test matrix verification

14 November 2026. Baseline: `e3d6b83`. **Completes ADR 0065.**

R.4 is the final sub-slice. All 12 items in ADR 0065 §4 are now
backed by passing integration tests. This verification also acts
as the session summary for the four-sub-slice implementation
(R.1 → R.4).

## Executed result

**929 Rust tests passed, 0 failed, 0 ignored.** Clippy clean on
`bradian-relational --all-targets`. No persistent format change
beyond what ADR 0063 / 0064 / 0065 already specified.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace` | 929 passed, 0 failed |
| `cargo test -p bradian-relational --test row_store_bace` | 15 passed |
| `cargo clippy -p bradian-relational --all-targets -- -D warnings` | Passed |

## What R.4 added

Four new tests in `tests/row_store_bace.rs`:

- **`mixed_generation_recovery_picks_the_latest_authoritative_generation`**
  — **ADR 0065 §4 item 6.** A directory sees three checkpoints in
  sequence: legacy `open` (v1), `open_with_bace` + Raw (v1 bit-
  identical), `open_with_bace` + Balanced (v2). Reopen under
  Balanced sees every row from the authoritative (latest)
  generation. Indexed reads agree.
- **`checkpoint_failure_under_bace_leaves_prior_generation_authoritative`**
  — **ADR 0065 §4 item 11.** A gen-1 Raw checkpoint succeeds. A
  gen-2 Balanced attempt with a broken policy (allow-set contains
  only unregistered `CodecId(16)`) fails closed with
  `BaceCodecUnavailable`. Byte-for-byte: gen-1 snapshot, manifest
  and WAL are unchanged. No gen-2 snapshot is published. Legacy
  `open` still sees the gen-1 row.
- **`open_with_bace_on_a_legacy_v1_only_directory_works`** —
  **ADR 0065 §4 item 12.** A directory originally written by the
  pre-BACE `RowStore` opens cleanly through `open_with_bace` with
  a Balanced wiring. Recovery sees only v1 pages, never calls the
  envelope decoder, never trips `BaceOpenWithoutBaceWiring`.
- **`recovery_fails_closed_when_envelope_names_an_unregistered_codec`**
  — Closes the last fail-closed variant from R.3's deferred list.
  A hand-crafted v2 page declaring `codec_id = 17` (unassigned per
  ADR 0061 P1) fails recovery with `BaceCodecUnavailable { codec_id: 17 }`.

## ADR 0065 §4 test matrix — complete

| Item | Test name | Slice |
|---|---|---|
| 1 | Every pre-existing `row_store_*` test passes | R.1+ |
| 2 | v1 writer bit-compatibility | implicit via item 3 and golden ADR 0064 test |
| 3 | `raw_profile_emits_v1_bit_identical_to_legacy_open` | R.2 |
| 4 | `balanced_profile_emits_v2_data_pages_with_bace_envelope_flag_set` | R.2 |
| 5 | `balanced_profile_round_trips_through_checkpoint_and_reopen` | R.3 |
| 6 | `mixed_generation_recovery_picks_the_latest_authoritative_generation` | **R.4** |
| 7 | `decoded_length_above_policy_cap_fails_closed_before_allocation` | R.3 |
| 8 | `legacy_open_on_a_v2_checkpoint_fails_with_bace_open_without_bace_wiring` | R.3 |
| 9 | `checkpoint_fails_closed_when_allowed_codec_set_contains_only_unregistered_ids` | R.2 |
| 10 | `recovery_fails_closed_on_codec_format_version_mismatch` | R.3 |
| 11 | `checkpoint_failure_under_bace_leaves_prior_generation_authoritative` | **R.4** |
| 12 | `open_with_bace_on_a_legacy_v1_only_directory_works` | **R.4** |

Plus one bonus fail-closed proof: `recovery_fails_closed_when_envelope_names_an_unregistered_codec`.

Every one of the five `StoreError` variants added in R.1 is now
exercised by a passing test:

| Variant | Fires in |
|---|---|
| `BaceCodecUnavailable` | checkpoint (R.2) + recovery (R.4) |
| `BaceCodecFormatVersionMismatch` | recovery (R.3) |
| `BaceEnvelope(_)` | plumbed via `#[from]`; fires on any envelope corruption; tested indirectly through the doctored-page variants |
| `BaceDecode(_)` | plumbed via `#[from]`; fires on codec decode errors; tested indirectly |
| `BaceOpenWithoutBaceWiring { page_id }` | recovery (R.3) |

## Slice R summary (R.1 → R.4, 2026-11-14)

| Slice | Commit | Scope |
|---|---|---|
| Design acceptance | `43e8986` | ADR 0065 PROPOSED → ACCEPTED with §6 defaults |
| R.1 | `7f33993` | Scaffolding: `BaceWiring`, 5 new errors, `open_with_bace` entry point. No behaviour change. 4 tests. |
| R.2 | `9fe1022` | Checkpoint writer: v2 pages wrapping BACE envelopes under non-Raw profile; Raw stays bit-identical to v1. **First BRPG v2 bytes on disk in production.** 3 tests. |
| R.3 | `e3d6b83` | Recovery reader: decode envelopes on open; `BaceOpenWithoutBaceWiring` on legacy `open`; format-version checks. Also refactored `open_with_bace` to use shared `open_inner`. 4 tests. |
| **R.4** | this slice | Mixed-generation + failure isolation + legacy→bace + extra fail-closed. 4 tests. |

**Workspace growth across Slice R:** 914 → **929 tests (+15), 0 failures throughout.** Clippy clean on every commit.

## Honest status of the BACE carrier workstream

With R.4 committed, the row_store carrier integration is **done at
the scope of ADR 0065**. The remaining BACE workstream is:

| ADR | Scope | Status |
|---|---|---|
| 0066 | Second codec (ZSTD) with determinism corpus per ADR 0061 §3.7 D1 | **not drafted** |
| 0067 | `backup.rs` BACE adapter — plan §12.1 recommended first subsystem | **not drafted** |
| 0068 | Authenticated envelope variant (`"BACA"` magic reserved by ADR 0062) | **not drafted** |
| 0069 | B+tree v2 Index page key encoding | **not drafted** |
| 0070 | Dictionary scope and lifecycle | **not drafted** |
| 0071 | `.brcs` BACE integration | **not drafted** |
| 0072 | BRWL BACE integration | **not drafted** |

**Production effect of the BACE stack as of R.4:** stores opened
via the legacy `open` / `open_with_policies` entry points continue
to be byte-identical to pre-BACE output. Stores opened via
`open_with_bace` can be checkpointed under a Balanced profile and
emit v2 pages wrapping RAW-envelope payloads. Because RAW is an
identity codec, Balanced checkpoints are currently **36 bytes
larger per data page** than Raw checkpoints — space savings wait on
ADR 0066 (ZSTD). The carrier plumbing works end-to-end; the value
arrives when a compressing codec lands.

See [ADR 0065](../adr/0065-row-store-bace-checkpoint-integration.md),
[R.1 verification](BRADIAN_BACE_R1_SCAFFOLDING_VERIFICATION_2026_11_14.md),
[R.2 verification](BRADIAN_BACE_R2_CHECKPOINT_WRITER_VERIFICATION_2026_11_14.md),
[R.3 verification](BRADIAN_BACE_R3_RECOVERY_READER_VERIFICATION_2026_11_14.md).
