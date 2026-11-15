# ADR 0078 — BACE dictionary primitive (implementation; **ACCEPTED**)

**Status: ACCEPTED, 15 November 2026.** Owner accepted with the
drafted defaults; implementation landed in the same commit.

Library-level primitive ONLY, mirroring ADR 0075 (authenticated
envelope) scope. Carrier integration (RowStore, `.brcs`, BRWL,
backup) is explicitly deferred to future ADRs that come with
workload benchmarks.

Implementation summary:

- Workspace pin unchanged; `crates/bradian-relational/Cargo.toml`
  enables the `zdict_builder` feature on the existing `zstd`
  dep (needed for `zstd::dict::from_samples`).
- `crates/bradian-relational/src/bace/dictionary.rs` — new file
  containing `BaceDictionary`, `DictionaryError`,
  `train_zstd_dictionary`, `encode_zstd_with_dictionary`,
  `decode_zstd_with_dictionary`, and 11 unit tests.
- `crates/bradian-relational/src/bace/mod.rs` — module
  declaration + public re-exports.

1026 workspace tests pass (1015 → 1026); clippy clean on
`cargo clippy --workspace --all-targets --locked -- -D warnings`.

## 1. Context

- BACE §4.2 reserves "dictionary scope" for columnar / row-store
  workloads that share a lot of byte-level prefix structure.
  Pre-trained dictionaries let ZSTD (and other dict-aware codecs)
  compress each individual page far smaller because the common
  byte sequences are stored once in the dictionary rather than
  repeated per page.
- The policy service already has a `DictionaryScope` enum
  (`None`, `Column`, `Table`, `Database`) and the
  `PolicyKey::CompressionDictionaryScope` key resolves it (see
  `bradian_relational::bace::policy`). But nothing in the codec,
  envelope, or any carrier consumes it yet. The flag
  `EnvelopeFlags::has_sidecar_dictionary` exists and is
  round-tripped but never set by a writer.
- ZSTD has first-class dictionary support
  (`zstd::dict::EncoderDictionary` / `DecoderDictionary`) and
  is already in the workspace (ADR 0066). A dictionary-aware
  encode/decode path is a library detail; it does NOT need a new
  codec ID because the envelope's existing `codec_id`
  (`ZSTD_ID = 49`) is still ZSTD — just invoked with a
  dictionary.
- Open design question that this ADR deliberately **does not
  answer**: how does a reader on one machine obtain the
  dictionary a writer on another machine used? Possible answers:
  sidecar file alongside the data, dictionary blob inside a
  reserved envelope field, external dictionary registry. Each
  has its own scope. v1 primitive stays agnostic: operators
  supply `&BaceDictionary` to encode/decode; later ADRs decide
  the transport.

## 2. Scope of this ADR

**In scope:**

- New module `bace::dictionary` in
  `crates/bradian-relational/src/bace/dictionary.rs`:
  - `BaceDictionary` struct:
    - Raw dictionary bytes (owned `Vec<u8>`).
    - 32-byte BLAKE3 fingerprint over the raw bytes
      (identity; used by future ADRs for dictionary-matching
      checks).
    - Human-readable label for error messages / logs.
    - `Debug` redacts the raw bytes, prints the label and the
      fingerprint's first 8 hex chars.
    - `PartialEq` uses constant-time fingerprint equality.
    - No public accessor for raw bytes — operators interact
      only via the encode/decode helpers in this module.
  - `train_zstd_dictionary(samples: &[&[u8]], target_bytes: usize,
    label: impl Into<String>) -> Result<BaceDictionary,
    DictionaryError>` — thin wrapper over `zstd::dict::from_samples`.
    "Training" means fitting a dictionary to a set of
    representative sample pages. Target size (bytes) bounds the
    output.
  - `encode_zstd_with_dictionary(input, dict, level, output) ->
    Result<usize, DictionaryError>` — ZSTD-dict-aware encode via
    `zstd::dict::EncoderDictionary`. Level pinned at 3 to match
    ADR 0066.
  - `decode_zstd_with_dictionary(input, dict, decoded_length,
    output) -> Result<(), DictionaryError>` — ZSTD-dict-aware
    decode via `zstd::dict::DecoderDictionary`. Decoded-length
    bound enforced the same way ADR 0066 enforces it (passed
    through to `zstd::bulk::decompress_to_buffer`).
  - `DictionaryError` enum: at minimum `TrainingFailed`,
    `EncodeFailed`, `DecodeFailed`, `DecodedLengthExceeded`,
    `DecodedLengthMismatch`, `InputTooLarge`.
- A `#[cfg(test)]`-only key accessor is permissible for
  reproducing the fingerprint from the raw bytes.
- 10+ unit tests:
  - Round-trip over trained + untrained samples.
  - Fingerprint determinism (same bytes → same fingerprint).
  - Decoded-length cap fires before buffer allocation.
  - Decoded-length mismatch fails closed.
  - Wrong-dictionary decode fails with `DecodeFailed` (NOT a
    silent incorrect result — this is the key safety invariant).
  - Training with no samples fails `TrainingFailed`.
  - Training target-size clamp (ZSTD requires a minimum dict
    size; the helper surfaces that via `TrainingFailed`).
  - Size-reduction demonstration: 100 redundant `.brcs`-style
    sample pages trained to a 16 KB dict, then the first page
    encoded with the dict is strictly smaller than the same
    page encoded without.
  - `Debug` redacts the dictionary bytes.
  - Fingerprint-based `PartialEq` constant-time check.

**Explicitly out of scope** (future ADRs):

- **Carrier integration.** No `RowStore::open_with_bace_dict`,
  no `SegmentWriter::new_with_bace_dict`, no BRWL dict wrap, no
  backup archive carries a dictionary. The envelope's
  `has_sidecar_dictionary` flag STAYS unused by writers.
- **Extending `BaceWiring` to carry an `Option<BaceDictionary>`.**
  A future carrier-integration ADR adds this; without carrier
  integration, the field would be dead.
- **Dictionary registry** mapping `dictionary_id → BaceDictionary`
  for cross-reference from envelopes. The primitive handles
  one dictionary passed by reference; registry is future work.
- **Envelope header changes** to carry a dictionary ID. The
  existing `has_sidecar_dictionary` bit stays; a new `u16`
  `dictionary_id` field would require the ADR 0075-reserved
  `BACA` magic or a new envelope version. Out of scope here.
- **CLI exposure.** `bradian-db` gains no new flags.
- **Dictionary rotation / versioning / lifecycle.** Operators
  are responsible for selecting the right dictionary at
  encode time; mismatched decode fails closed. A later ADR
  adds generational semantics.
- **Automatic dictionary selection based on `DictionaryScope`
  policy value.** The policy enum exists; routing is future
  work.
- **Non-ZSTD dictionary codecs.** RAW has no dictionary;
  future codecs may add their own helpers.
- **Trained-dictionary persistence format.** v1 stores raw
  bytes + label in memory; ADR does not define an on-disk
  dictionary file format.
- **Confidentiality / integrity of the dictionary itself.**
  The fingerprint provides integrity-at-rest if a future
  ADR records it alongside the dict. BACA-wrapping a
  dictionary is a separate ADR.

## 3. Proposed design

### 3.1 `BaceDictionary` type

```rust
#[derive(Clone)]
pub struct BaceDictionary {
    bytes: Vec<u8>,
    fingerprint: [u8; 32],   // BLAKE3 of bytes
    label: String,
}

impl BaceDictionary {
    pub fn new(bytes: Vec<u8>, label: impl Into<String>) -> Self {
        let fingerprint = *blake3::hash(&bytes).as_bytes();
        Self { bytes, fingerprint, label: label.into() }
    }
    pub fn label(&self) -> &str { &self.label }
    pub fn fingerprint(&self) -> &[u8; 32] { &self.fingerprint }
    pub fn len(&self) -> usize { self.bytes.len() }
    pub fn is_empty(&self) -> bool { self.bytes.is_empty() }

    #[cfg(test)]
    pub(crate) fn bytes_for_test(&self) -> &[u8] { &self.bytes }
}

impl std::fmt::Debug for BaceDictionary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // ADR 0078 §3.1 — Debug redacts the raw bytes. Prints
        // label + first 8 fingerprint hex chars so operators
        // can correlate debug output with the dictionary identity
        // without seeing (or logging) the dictionary content.
        let fp_short = format!(
            "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            self.fingerprint[0], self.fingerprint[1], self.fingerprint[2],
            self.fingerprint[3], self.fingerprint[4], self.fingerprint[5],
            self.fingerprint[6], self.fingerprint[7],
        );
        f.debug_struct("BaceDictionary")
            .field("label", &self.label)
            .field("fingerprint_prefix", &fp_short)
            .field("len", &self.bytes.len())
            .field("bytes", &"[redacted]")
            .finish()
    }
}

impl PartialEq for BaceDictionary {
    fn eq(&self, other: &Self) -> bool {
        // Constant-time fingerprint equality.
        self.fingerprint.ct_eq(&other.fingerprint).unwrap_u8() == 1
    }
}
impl Eq for BaceDictionary {}
```

### 3.2 Training helper

```rust
pub fn train_zstd_dictionary(
    samples: &[&[u8]],
    target_bytes: usize,
    label: impl Into<String>,
) -> Result<BaceDictionary, DictionaryError> {
    if samples.is_empty() {
        return Err(DictionaryError::TrainingFailed(
            "at least one sample required".into(),
        ));
    }
    let bytes = zstd::dict::from_samples(samples, target_bytes)
        .map_err(|e| DictionaryError::TrainingFailed(e.to_string()))?;
    Ok(BaceDictionary::new(bytes, label))
}
```

### 3.3 Encode / decode helpers

```rust
pub fn encode_zstd_with_dictionary(
    input: &[u8],
    dict: &BaceDictionary,
    output: &mut Vec<u8>,
) -> Result<usize, DictionaryError> {
    if input.len() > u32::MAX as usize {
        return Err(DictionaryError::InputTooLarge {
            bytes: input.len(), limit: u32::MAX,
        });
    }
    // Level pinned at 3 to match ADR 0066 ZSTD baseline.
    let enc_dict = zstd::dict::EncoderDictionary::copy(dict.bytes_for_dict(), 3);
    // Use zstd::bulk::Compressor so we can bound output.
    let mut compressor = zstd::bulk::Compressor::with_prepared_dictionary(&enc_dict)
        .map_err(|e| DictionaryError::EncodeFailed(e.to_string()))?;
    let compressed = compressor.compress(input)
        .map_err(|e| DictionaryError::EncodeFailed(e.to_string()))?;
    output.extend_from_slice(&compressed);
    Ok(compressed.len())
}

pub fn decode_zstd_with_dictionary(
    input: &[u8],
    dict: &BaceDictionary,
    decoded_length: u32,
    output: &mut Vec<u8>,
) -> Result<(), DictionaryError> {
    let dec_dict = zstd::dict::DecoderDictionary::copy(dict.bytes_for_dict());
    let mut decompressor = zstd::bulk::Decompressor::with_prepared_dictionary(&dec_dict)
        .map_err(|e| DictionaryError::DecodeFailed(e.to_string()))?;
    let out_cap = decoded_length as usize;
    let decoded = decompressor.decompress(input, out_cap)
        .map_err(|_| DictionaryError::DecodedLengthExceeded {
            bytes: usize::MAX, declared: decoded_length,
        })?;
    if decoded.len() as u64 != decoded_length as u64 {
        return Err(DictionaryError::DecodedLengthMismatch {
            bytes: decoded.len(), declared: decoded_length,
        });
    }
    output.extend_from_slice(&decoded);
    Ok(())
}
```

`bytes_for_dict()` is a `pub(crate)` accessor (NOT public —
mirrors ADR 0075's `BaceAuthentication::key_bytes`).

### 3.4 Error variants

```rust
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DictionaryError {
    #[error("ZSTD dictionary training failed: {0}")]
    TrainingFailed(String),
    #[error("ZSTD dictionary encode failed: {0}")]
    EncodeFailed(String),
    #[error("ZSTD dictionary decode failed: {0}")]
    DecodeFailed(String),
    #[error("input {bytes} exceeds limit {limit}")]
    InputTooLarge { bytes: usize, limit: u32 },
    #[error("decoded output {bytes} exceeds declared decoded_length {declared}")]
    DecodedLengthExceeded { bytes: usize, declared: u32 },
    #[error("decoded output {bytes} does not match declared decoded_length {declared}")]
    DecodedLengthMismatch { bytes: usize, declared: u32 },
}
```

### 3.5 Compliance obligations (implementation test checklist)

1. Training with one sample + target 16 KB yields a non-empty
   dictionary whose fingerprint is deterministic.
2. Training with zero samples fails `TrainingFailed`.
3. Encode/decode round-trip with matching dictionary preserves
   bytes exactly.
4. Decode with a wrong (non-matching) dictionary fails
   `DecodeFailed` — NOT a silent incorrect-byte result.
5. Decode with a `decoded_length` cap below the real decoded
   size fails `DecodedLengthExceeded` before allocation.
6. Decode where real output doesn't match declared length
   fails `DecodedLengthMismatch`.
7. Fingerprint determinism: `BaceDictionary::new(bytes, "a")`
   and `::new(bytes, "b")` have equal fingerprints
   (fingerprint depends only on bytes).
8. `PartialEq` returns `true` for equal fingerprints and
   `false` otherwise.
9. `Debug` string contains the label and fingerprint prefix
   but NOT the dictionary bytes.
10. **Size-reduction demonstration:** build 100 sample pages
    sharing a common 100-byte prefix. Train a dict. Encode one
    sample with the dict and without. The dict-encoded output
    is strictly smaller.
11. Input larger than `u32::MAX` fails `InputTooLarge` fast.

## 4. Alternatives considered

**A. Add a new codec ID (`ZSTD_WITH_DICT = CodecId(50)`) that
carries dictionary state internally.** Cleaner codec trait
semantics. Rejected: the dictionary is a runtime parameter,
not a codec choice; operators choose "ZSTD + this dict" at
encode time, same codec ID. Adding a parallel ID would double
the registry surface.

**B. Extend the `Codec` trait with `encode_with_dict` /
`decode_with_dict` optional methods.** Cleaner. Rejected for
v1: only ZSTD will have dictionary support in the foreseeable
future; a trait extension for one implementor is premature.
This ADR puts helpers in a separate module; a future ADR can
refactor into the trait if a second dict-aware codec arrives.

**C. Make `BaceDictionary` implement `Codec` directly.**
Simpler. Rejected: the `Codec` trait's encode/decode take no
extra parameters; a dict-aware Codec would need a different
trait, or the dict baked into the type's state. Either widens
scope beyond the primitive.

**D. Store the dictionary bytes publicly (`pub fn bytes()`).**
Simpler API. Rejected per ADR 0075 pattern: the type
discourages casual access to the raw bytes; callers interact
through the helpers.

**E. Ship carrier integration (`.brcs` per-column dict) in the
same ADR.** Rejected: `.brcs` dictionary integration would
require a `.brcs` format change AND a dictionary transport
decision. Separate ADR.

**F. Hash the label into the fingerprint so different labels
with the same bytes compare unequal.** Rejected: fingerprint
represents dictionary *identity* (what matters for decode
correctness); label is cosmetic. Mixing them creates
false-mismatch bugs.

## 5. Open design questions (owner review)

1. **`#[cfg(test)]` bytes accessor.** Draft: `bytes_for_test`
   is test-only, `bytes_for_dict` is `pub(crate)` for the
   encode/decode helpers in the same module. Confirm the
   two-tier visibility.
2. **Compression level.** Draft: pinned at ZSTD level 3 to
   match ADR 0066 ZstdCodec. Confirm.
3. **Target training size default.** Draft: no default — the
   caller must pass a target. Alternatives: 16 KB default,
   64 KB default. Confirm "no default".
4. **Fingerprint primitive.** Draft: BLAKE3 unkeyed hash.
   Alternatives: SHA-256. Confirm BLAKE3 (consistent with ADR
   0075).
5. **Carrier integration deferral.** Draft: NOT in this ADR.
   Confirm.
6. **Exposing training through the CLI.** Draft: NOT in this
   ADR. Confirm.
7. **`DictionaryError::DecodeFailed(String)` — leaking ZSTD
   error messages.** Draft: YES, operators need the detail
   for diagnosis. Alternative: opaque "decode failed".
   Confirm.

## 6. Impact on invariants

- **INV-001 commit correctness:** N/A — no carrier integration.
- **INV-002 one authoritative state:** N/A.
- **INV-003 no authorization gap:** N/A. The primitive adds no
  authorization enforcement.
- **INV-004 no unsafe automatic availability:** N/A.
- **INV-005 recoverable lifecycle:** N/A for this primitive.
- **CLAUDE.md §3:** uses audited `zstd::dict` and `blake3` —
  no custom crypto or compression. ✓

## 7. Dependencies this ADR would add

None. `bradian-relational` already deps on `blake3` (ADR 0075)
and `zstd` (ADR 0066). The `zstd::dict` module is part of the
existing `zstd` crate.

## 8. What this ADR explicitly does NOT do

- Touch any carrier
  (`RowStore`, `.brcs`, BRWL, backup, lifecycle, SQL engines).
- Extend `BaceWiring`.
- Add CLI exposure.
- Add a new codec ID.
- Make a production feature claim about dictionary-scope
  compression. That requires a carrier-integration ADR that
  consumes this primitive and comes with benchmarks on the
  carrier's workload.
- Define a persistent on-disk dictionary file format.
- Address dictionary rotation, versioning, or registry.

## 9. Acceptance evidence required before marking Accepted

1. ADRs 0060–0077 accepted — **satisfied**.
2. Owner sign-off on §5 open design questions.
3. Implementation satisfying every item in §3.5.
4. Review-level inspection confirming:
   - MAC-like primitive uses `blake3::hash` (unkeyed
     fingerprint is fine; this is identity, not
     authentication).
   - ZSTD dictionary path uses `zstd::dict::from_samples`,
     `zstd::dict::EncoderDictionary::copy`,
     `zstd::dict::DecoderDictionary::copy` — no custom
     dictionary arithmetic.
   - `BaceDictionary::Debug` does not print raw bytes
     (manual inspection + test 9).
5. No regressions in any existing `bradian-relational` test.

Only then can this ADR move from **PROPOSED** to **ACCEPTED**.
This ADR landing does NOT constitute shipping dictionary-mode
compression as an operator feature; the feature ships when
a later carrier-integration ADR consumes this primitive and
records benchmarks on a realistic carrier workload.
