# BACE dictionary primitive verification (ADR 0078)

15 November 2026. Baseline: `6ba7c5c`. Library-level
`BaceDictionary` + ZSTD-dictionary-aware encode/decode
helpers. Mirrors ADR 0075 pattern: ships only the primitive;
carrier integration is explicitly deferred.

**This ADR does NOT ship dictionary-mode compression as an
operator feature.** A future carrier-integration ADR will
consume this primitive with workload benchmarks before any
feature claim.

## Executed result

**1026 Rust tests passed, 0 failed, 0 ignored** (1015 → 1026).
Clippy clean on `cargo clippy --workspace --all-targets
--locked -- -D warnings`.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace --locked` | 1026 passed, 0 failed |
| `cargo test -p bradian-relational --lib dictionary` | 11 passed |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed |

## What landed

### `crates/bradian-relational/Cargo.toml`

The `zdict_builder` feature of the existing `zstd` dep is now
enabled:

```toml
zstd = { workspace = true, features = ["zdict_builder"] }
```

`zstd::dict::from_samples` is behind this feature flag.
Workspace pin unchanged.

### `crates/bradian-relational/src/bace/dictionary.rs` (new)

```rust
pub struct BaceDictionary {
    bytes: Vec<u8>,             // private
    fingerprint: [u8; 32],      // BLAKE3 over bytes
    label: String,
}

impl BaceDictionary {
    pub fn new(bytes: Vec<u8>, label: impl Into<String>) -> Self;
    pub fn label(&self) -> &str;
    pub fn fingerprint(&self) -> &[u8; 32];
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub(crate) fn bytes_for_dict(&self) -> &[u8];     // crate-internal
    #[cfg(test)] pub(crate) fn bytes_for_test(&self) -> &[u8];
}

impl Debug for BaceDictionary { /* redacts bytes; prints label + fp prefix */ }
impl PartialEq for BaceDictionary { /* constant-time fingerprint eq */ }
impl Eq for BaceDictionary {}

pub enum DictionaryError {
    TrainingFailed(String),
    EncodeFailed(String),
    DecodeFailed(String),
    InputTooLarge { bytes: usize, limit: u32 },
    DecodedLengthExceeded { declared: u32 },
    DecodedLengthMismatch { bytes: usize, declared: u32 },
}

pub fn train_zstd_dictionary(
    samples: &[&[u8]], target_bytes: usize, label: impl Into<String>,
) -> Result<BaceDictionary, DictionaryError>;

pub fn encode_zstd_with_dictionary(
    input: &[u8], dict: &BaceDictionary, output: &mut Vec<u8>,
) -> Result<usize, DictionaryError>;

pub fn decode_zstd_with_dictionary(
    input: &[u8], dict: &BaceDictionary, decoded_length: u32, output: &mut Vec<u8>,
) -> Result<(), DictionaryError>;
```

Key implementation points:

- **Fingerprint is unkeyed BLAKE3** over the raw dictionary
  bytes. Identity primitive; not authentication.
- **Fingerprint depends only on bytes, not label.** Two
  dictionaries with the same bytes but different labels
  compare equal (tests item 7 + `fingerprint_depends_only_on_bytes_not_label`).
- **Constant-time fingerprint equality** via
  `subtle::ConstantTimeEq`.
- **No public raw-bytes accessor.** Mirrors ADR 0075's
  `BaceAuthentication::key_bytes` — callers interact only
  through the encode/decode helpers.
- **Level pinned at 3** for the encode path, matching ADR
  0066 ZstdCodec baseline.
- **Bounded decode** via
  `zstd::bulk::Decompressor::decompress(input, out_cap)`
  where `out_cap = decoded_length as usize`. The library
  enforces the bound during decoding, consistent with ADR
  0066 BACE-FMT-003 semantics.
- **Mismatched-dictionary decode fails closed** (test item 4
  — the key safety invariant).

### `crates/bradian-relational/src/bace/mod.rs`

```rust
pub mod dictionary;
pub use dictionary::{
    decode_zstd_with_dictionary, encode_zstd_with_dictionary,
    train_zstd_dictionary, BaceDictionary, DictionaryError,
};
```

### 11 unit tests

| # | Test | §3.5 item |
|---|---|---|
| 1 | `item_1_training_yields_non_empty_deterministic_dict` | 1 + 7 (combined) |
| 2 | `item_2_training_with_zero_samples_fails` | 2 |
| 3 | `item_3_encode_decode_round_trip_preserves_bytes` | 3 |
| 4 | `item_4_decode_with_wrong_dict_fails_decode_failed_not_silent` | 4 — safety invariant |
| 5 | `item_5_decoded_length_cap_fires_before_allocation` | 5 |
| 6 | `item_6_decoded_length_mismatch_fails_closed` | 6 |
| 7 | `item_8_partial_eq_uses_fingerprint` | 8 |
| 8 | `item_9_debug_redacts_raw_bytes` | 9 |
| 9 | `item_10_size_reduction_demonstration` | 10 |
| 10 | `item_11_input_too_large_fast_fail` | 11 |
| 11 | `fingerprint_depends_only_on_bytes_not_label` | 7 (explicit cross-check) |

### §3.5 item 10 size-reduction demonstration

Trained a 8 KB dictionary on 128 `.brcs`-style sample pages
with a common 28-byte prefix (`BRADIAN-SAMPLE-COMMON-PREFIX-`)
plus a per-sample numeric suffix and a 50-byte redundant
tail. Encoded the first sample with and without the dict:

- `with_dict` is **strictly smaller** than `without_dict`
  (plain `zstd::bulk::compress(input, 3)`).
- The test asserts the strict inequality; the exact bytes
  depend on ZSTD version and are not pinned.

## Compliance against ADR 0078 §9 acceptance evidence

1. ADRs 0060–0077 accepted — ✓
2. §5 open design questions landed per drafted defaults:
   - Q1 (two-tier visibility): `bytes_for_dict` is
     `pub(crate)`, `bytes_for_test` is `#[cfg(test)]
     pub(crate)` — ✓
   - Q2 (level 3) — ✓
   - Q3 (no training-size default) — ✓, no `Default` on
     the trainer
   - Q4 (BLAKE3) — ✓
   - Q5 (carrier deferral) — ✓
   - Q6 (no CLI) — ✓
   - Q7 (leak ZSTD errors for diagnosis) — ✓ via
     `TrainingFailed(String)` / `EncodeFailed(String)` /
     `DecodeFailed(String)`
3. Every §3.5 item 1–11 covered by one or more tests.
4. Review-level inspection:
   - MAC-like primitive uses `blake3::hash` — ✓
   - ZSTD dictionary path uses `zstd::dict::from_samples`,
     `EncoderDictionary::copy`,
     `DecoderDictionary::copy` — ✓
   - `BaceDictionary::Debug` does not print raw bytes — ✓
     (test item 9).
5. No regressions in any existing `bradian-relational` test
   — ✓.

## What this ADR explicitly does NOT do (unchanged)

- Touch any carrier (`RowStore`, `.brcs`, BRWL, backup,
  lifecycle, SQL engines).
- Extend `BaceWiring`.
- Add CLI exposure.
- Add a new codec ID.
- Define a persistent on-disk dictionary file format.
- Address dictionary rotation, versioning, or registry.
- Make a production feature claim about dictionary-mode
  compression.

## BACE workstream progress (as of this ADR)

| ADR | Component | Status |
|---|---|---|
| 0060–0066 | Prerequisites + RowStore + ZSTD | ACCEPTED |
| 0067–0068 | Backup carrier adapters | ACCEPTED |
| 0069–0072 | All four SQL engines BACE open | ACCEPTED |
| 0073 | `bradian-db` CLI BACE flags | ACCEPTED |
| 0074 | G23 DDL demonstration | ACCEPTED |
| 0075 | BACA envelope primitive | ACCEPTED |
| 0076 | `.brcs` carrier BACE | ACCEPTED |
| 0077 | BRWL carrier BACE (benchmarked) | ACCEPTED |
| **0078** | **BACE dictionary primitive** | **ACCEPTED** |

## Remaining BACE workstream candidates

The two remaining candidates both require consuming
primitives from the shipped ADRs and come with explicit
evidence obligations:

- **Carrier integration of `BaceAuthentication` (ADR 0075)**
  — the concrete tamper-evidence feature. Requires full
  CLAUDE.md §6 evidence package (isolation, revocation,
  audit integrity, key rotation, backup protection, incident
  response).
- **Carrier integration of `BaceDictionary` (ADR 0078)** —
  per-column dictionary consumption in `.brcs` and/or BRWL.
  Requires workload benchmarks + a dictionary-transport
  decision (sidecar, inline envelope, external registry).

Both are substantial; each should be its own ADR with its own
scope-boundary discussion.

## Baseline + commit

Baseline: `bae2244` (ADR 0078 PROPOSED).
Implementation commit: assigned by this commit.

See [ADR 0078](../adr/0078-bace-dictionary-primitive.md).
