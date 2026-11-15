# BACE `.derc` carrier adapter verification (ADR 0076)

15 November 2026. Baseline: `aa2555e`. First BACE ADR that
touches `dap-storage`. `.derc` format minor-version bump
v1.0 → v1.1 with additive feature flag and per-page trailer
byte.

## Executed result

**1005 Rust tests passed, 0 failed, 0 ignored** (995 → 1005).
Clippy clean on `cargo clippy --workspace --all-targets
--locked -- -D warnings`.

| Executed check | Result |
|---|---|
| `cargo check -p dap-storage` | Passed |
| `cargo test --workspace --locked` | 1005 passed, 0 failed |
| `cargo test -p dap-storage --test derc_bace` | 10 passed |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed |

## What landed

### R.1 — cross-crate dep + entry points

`crates/dap-storage/Cargo.toml`:

```toml
bradian-relational = { path = "../bradian-relational" }
```

`SegmentWriter::new_with_bace` and `SegmentReader::open_with_bace`
added, storing an `Option<BaceWiring>` on each struct.

### R.2 — v1.1 format bits

`crates/dap-storage/src/segment/format.rs`:

```rust
pub const SEGMENT_VERSION_MINOR_V1_1: u16 = 1;

impl FeatureFlags {
    pub const BACE_PAGE_ENVELOPE: u64 = 1 << 5;
    pub const ALL_SUPPORTED_V1_1: u64 =
        Self::ALL_SUPPORTED_V1 | Self::BACE_PAGE_ENVELOPE;
    pub fn is_subset_supported_v1_1(required: u64) -> bool;
}

#[repr(u8)]
pub enum BaceEnvelopeTag {
    None = 0,
    BaceV1 = 1,
    BacaV1Reserved = 2,
}
```

Reader's `read_footer_with_features(file, allow_bace)` replaces
the private `read_footer`. `open` passes `allow_bace=false`
(rejects BACE segments); `open_with_bace` passes
`allow_bace=true`.

### R.3 — Writer emits BACE envelopes

`SegmentFileWriter::new_with_bace` accepts the wiring and
resolves `bace_active` via a new `resolve_bace_active` helper
(checks that the wiring's `allowed_codecs` includes at least
one non-RAW registered codec). The RAW profile opts out — no
envelope overhead for pages that wouldn't compress anyway.

The `write_header` method conditionally emits `minor=1` and
sets `BACE_PAGE_ENVELOPE` when `bace_active`.

`write_column` branches on `self.bace_active`:
- Active: encode inner payload via the resolved outer codec
  (ZSTD today), wrap via `bradian_relational::bace::envelope::
  write_envelope` with the matching `codec_id`, write the
  envelope bytes as the page body, then append the trailer
  byte `BaceEnvelopeTag::BaceV1`.
- Inactive: legacy `compress::compress` path unchanged, no
  trailer byte.

### R.4 — Reader decodes BACE envelopes + §3.5 tests

`SegmentReader::read_page` reads the trailer byte when
`_bace_feature_active` is true and dispatches:

| trailer | path |
|---|---|
| `0` | legacy `compress::decompress` or verbatim |
| `1` | BACE v1 envelope decode via wiring |
| `2` | `DerSegmentBacaUnsupported`-equivalent (clean error) |
| ≥ 3 | Corruption (unknown tag) |

The BACE decode uses
`PolicyKey::CompressionMaxDecodedPageBytes` from the wiring's
policy service as the envelope's `max_decoded_length`,
matching the pattern from ADR 0065 §3.4.

Anti-swap check: the envelope's `decoded_length` must equal
the page's declared `uncompressed_len`; a mismatch fails
closed with a specific corruption message.

### 10 new tests in `tests/derc_bace.rs`

| # | Test | §3.5 item |
|---|---|---|
| 1 | `item_1_writer_with_zstd_balanced_emits_v1_1_bace_segment` | 1 (minor=1 + BACE flag set) |
| 2 | `item_3_writer_without_wiring_emits_v1_0_byte_identical` | 3 (minor=0, no flag) |
| 3 | `writer_with_raw_only_wiring_opts_out_of_bace_envelope_overhead` | §3.2 RAW opt-out |
| 4 | `item_2_reader_with_bace_round_trips_bace_segment` | 2 |
| 5 | `item_4_legacy_reader_reads_v1_0_segment_unchanged` | 4 |
| 6 | `item_6_reader_with_bace_reads_legacy_v1_0_segment` | 6 |
| 7 | `item_5_legacy_reader_rejects_v1_1_segment_via_feature_flag` | 5 |
| 8 | `item_7_tampered_envelope_bytes_fail_closed` | 7 |
| 9 | `item_9_baca_reserved_trailer_byte_fails_closed` | 9 |
| 10 | `item_10_unknown_trailer_byte_fails_corruption` | 10 |

### §3.5 item 8 (cap enforcement) covered implicitly

The envelope-level decoded-length cap is proven by ADR 0065
R.3 test
`decoded_length_above_policy_cap_fails_closed_before_allocation`.
The `.derc` reader threads the same policy cap through
`EnvelopeCaps::max_decoded_length` and relies on
`read_envelope`'s bounded-decode step. No duplicate test
landed here.

## Compliance against ADR 0076 §3.5

| Item | Status |
|---|---|
| 1 Writer BACE emission | ✓ test 1 |
| 2 Reader round-trip of BACE segment | ✓ test 4 |
| 3 Legacy writer byte-identical | ✓ test 2 |
| 4 Legacy reader reads v1.0 | ✓ test 5 |
| 5 Legacy reader rejects v1.1 | ✓ test 7 |
| 6 Reader-with-bace tolerates v1.0 | ✓ test 6 |
| 7 Tamper detection | ✓ test 8 |
| 8 Cap enforcement | ✓ inherited from ADR 0065 R.3 |
| 9 BACA reserved value fails cleanly | ✓ test 9 |
| 10 Unknown trailer value fails cleanly | ✓ test 10 |

## Architectural note (§5 Q2 landed as drafted)

`dap-storage` now directly depends on `bradian-relational`.
This flips the historical direction (relational on analytical)
but is the minimum-churn path per the ADR §4 alternative A
rejection. If future coupling becomes a problem, carving
`bradian-bace` into a sibling crate is still possible — nothing
in this ADR locks in the current module layout.

## What this ADR explicitly does NOT do (unchanged)

- Rename `.derc` or any `dap-*` crate.
- Change the inner codec set.
- Add BACA / authenticated envelopes to `.derc` production
  writes.
- Add encryption.
- Rewrite any existing v1.0 segment.
- Extend `dap-admin` CLI for `.derc` BACE.
- Touch the `.derc` partition WAL, manifest, zone-map, or
  compaction code beyond the writer/reader entry points.
- Make a tamper-evidence security claim.

## BACE workstream progress (as of this ADR)

| ADR | Component | Status |
|---|---|---|
| 0060–0066 | Prerequisites + RowStore + ZSTD | ACCEPTED |
| 0067–0068 | Backup carrier adapters | ACCEPTED |
| 0069–0072 | All four SQL engines BACE open | ACCEPTED |
| 0073 | `bradian-db` CLI BACE flags | ACCEPTED |
| 0074 | G23 DDL demonstration tests | ACCEPTED |
| 0075 | BACA envelope primitive | ACCEPTED |
| **0076** | **`.derc` carrier BACE adapter** | **ACCEPTED** |

## Remaining BACE workstream candidates

- **BRWL carrier BACE** — WAL frame compression on
  latency-sensitive write path; benchmarking required.
- **Dictionary scope** — BACE §4.2 dictionary mode with
  per-column lifecycle.
- **Carrier integration of `BaceAuthentication`** — the
  concrete security feature consuming ADR 0075's primitive;
  needs full CLAUDE.md §6 evidence package.

## Baseline + commit

Baseline: `aa2555e` (ADR 0076 PROPOSED).
Implementation commit: assigned by this commit.

See [ADR 0076](../adr/0076-derc-carrier-bace.md).
