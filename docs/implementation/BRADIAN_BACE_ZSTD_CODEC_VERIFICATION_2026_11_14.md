# BACE ZSTD codec verification (ADR 0066)

14 November 2026. Baseline before: `8d0341a`. First compressing codec
registered in the BACE registry. RAW remains the critical fallback;
ZSTD ships at `CodecId(49)` and is operator-opt-in via
`allowed_codecs = [ZSTD_ID, RAW_ID]`.

## Executed result

**940 Rust tests passed, 0 failed, 0 ignored, across 101 suites.**
Clippy clean on `bradian-relational --all-targets`.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace --locked` | 940 passed, 0 failed |
| `cargo test -p bradian-relational --test row_store_bace` | 19 passed |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## What landed

### `crates/bradian-relational/Cargo.toml`

Added `zstd = { workspace = true }` as a direct dep. Workspace pin
is unchanged (`zstd = "0.13"`, `default-features = false`); already
on the Cargo.lock graph via `dap-storage`.

### `crates/bradian-relational/src/bace/codec.rs`

New inline module `pub mod zstd` paralleling the existing `raw`
module:

```rust
pub const ZSTD_ID: CodecId = CodecId(49);
pub const ZSTD_FORMAT_VERSION: u16 = 1;
pub const ZSTD_LEVEL: i32 = 3;        // matches ZSTD_CLEVEL_DEFAULT and .derc default
```

Capability descriptor matches ADR 0066 §3.2 exactly:

- `determinism: Determinism::HardwareDependent` (ADR 0061 §5 D1)
- `flags.critical = false` (RAW stays the fallback)
- `flags.allows_outer_compression = false` (ZSTD IS an outer compressor)
- `flags.preserves_null_distinct = true`

`encode()` uses `zstd::bulk::compress(input, 3)` — the ZSTD **frame**
format (not raw blocks). `decode()` passes the envelope-declared
`decoded_length` as the output capacity to
`zstd::bulk::decompress`, so the library itself enforces the bound
**during** decoding per BACE-FMT-003.

### `crates/bradian-relational/src/bace/registry.rs`

```rust
impl BuiltInRegistry {
    pub fn new() -> Self {
        Self::with_codecs(vec![Box::new(RawCodec), Box::new(ZstdCodec)])
    }
}
```

The existing tests that assumed a single-codec registry were
updated to the two-codec reality:

- `default_registry_registers_raw_and_zstd` — now checks both are
  registered with the correct capability flags.
- `negotiate_identical_sets_agrees_on_every_id_and_misses_none` —
  agreed set is now `[RAW_ID, ZSTD_ID]`.
- The remaining three negotiation tests work unchanged (ZSTD being
  non-critical means its absence on the remote side never puts it
  in `missing_critical`).

### New tests (10 total)

**6 unit tests in `src/bace/codec.rs`:**

- `zstd_codec_identifies_itself_as_id_49_hardware_dependent_non_critical_outer`
- `zstd_pinned_level_matches_derc_default` — asserts `ZSTD_LEVEL == 3`.
- `zstd_round_trip_empty_input_golden_corpus` — ADR 0066 §3.6 case 1.
- `zstd_round_trip_short_incompressible_golden_corpus` — §3.6 case 2.
- `zstd_round_trip_long_repetitive_golden_corpus_actually_saves_space`
  — §3.6 case 3. Asserts 4 000 copies of `b'x'` compress to under
  100 bytes AND round-trip byte-exactly.
- `zstd_decoder_fails_closed_when_declared_decoded_length_is_too_small`
  — §3.4 bound enforcement.
- `zstd_decoder_fails_closed_when_declared_decoded_length_disagrees_with_frame`
  — §3.4 post-decode mismatch check.

**4 integration tests in `tests/row_store_bace.rs`:**

- `zstd_balanced_profile_round_trips_through_checkpoint_and_reopen`
  — operator wiring with `[ZSTD_ID, RAW_ID]`, write → checkpoint →
  reopen → `get` and `indexed_get` every row.
- `zstd_balanced_profile_emits_v2_data_pages_declaring_codec_id_49`
  — inspects raw snapshot bytes, confirms envelope magic at page
  offset 32 and `codec_id = 49` in the header.
- `zstd_balanced_profile_envelope_encoded_length_is_smaller_than_decoded_on_redundant_data`
  — §3.8 test item 5 revised. The honest demonstration: at the
  envelope layer, redundant input gives `encoded_length <
  decoded_length / 10`. Physical snapshot-file savings would
  require variable-size pages (future ADR).
- `zstd_round_trip_survives_indexed_range_reads` — exercises the
  `indexed_range` path (streaming B+tree reads) on ZSTD-wrapped
  pages.

## Compliance against ADR 0066 §3.8 test checklist

| # | Checklist item | Status |
|---|---|---|
| 1 | `BuiltInRegistry::new()` registers ZSTD at ID 49 | ✓ `default_registry_registers_raw_and_zstd` |
| 2 | ZSTD capability descriptor matches §3.2 | ✓ `zstd_codec_identifies_itself_as_id_49_…` |
| 3 | Round-trip of empty, short-incompressible, long-repetitive golden-corpus inputs | ✓ three unit tests |
| 4 | Round-trip through the BACE envelope | ✓ `zstd_balanced_profile_round_trips_through_checkpoint_and_reopen` exercises `write_envelope` → `read_envelope` → decode via the full RowStore path |
| 5 | `balanced_profile_with_zstd_saves_space_on_repetitive_data` | **Revised** — physical snapshot savings are not reachable through per-page compression at fixed 4 KiB page size; the shipped test instead asserts envelope-level `encoded_length < decoded_length / 10` on redundant data |
| 6 | Fail-closed on declared-length-mismatch envelope | ✓ `zstd_decoder_fails_closed_when_declared_decoded_length_disagrees_with_frame` |
| 7 | Existing `tests/row_store_bace.rs` tests still pass | ✓ all 15 prior tests + 4 new = 19 pass |

## What this ADR explicitly does NOT do (unchanged from draft)

- Change the default `PlatformBaseline::allowed_codecs`. It stays
  `[RAW_ID]`. Operators opt in to ZSTD by passing
  `allowed_codecs = [ZSTD_ID, RAW_ID]` on the wiring. No existing
  BACE opt-in store silently switches format.
- Expose `compression.zstd_level` as a policy key. Level 3 is
  hardcoded; a later ADR adds parameterization.
- Implement BACE-SEL-002 avoid-expansion selection. On
  incompressible data ZSTD today still pays 36 bytes of envelope
  overhead.
- Touch `.derc`, BRWL, backup, or any other carrier.
- Use ZSTD dictionaries.

## Known design-level limitation surfaced by this slice

Per-page compression at a fixed 4 KiB physical page size does not
reduce snapshot-file size on disk — the compression savings are
visible only in the envelope header. Shipping true BACE-GOAL-001
physical savings at the RowStore layer requires one of:

- Variable-size BRPG data pages (requires page-format v3).
- Multi-logical-page coalescing into one physical page.
- Moving compression to the checkpoint-file level (wrap the whole
  snapshot) rather than per-page.

None of these are in ADR 0066 scope. A follow-up ADR will pick one.

## Baseline + commit

Baseline: `8d0341a` (ADR 0066 PROPOSED).
Implementation commit: `<will be assigned by this commit>`.

See [ADR 0066](../adr/0066-bace-zstd-codec.md).
