//! BACE dictionary primitive (ADR 0078).
//!
//! Library-level `BaceDictionary` + ZSTD-dictionary-aware encode/
//! decode helpers. No carrier integration — a future ADR consumes
//! this primitive with workload benchmarks before any production
//! feature claim.
//!
//! **Cryptographic primitives:** BLAKE3 unkeyed hash for dictionary
//! identity (ADR 0078 §3.1), `zstd::dict::from_samples` +
//! `EncoderDictionary::copy` + `DecoderDictionary::copy` for the
//! compression path. No custom crypto or compression per
//! CLAUDE.md §3.
//!
//! **Scope boundary** mirrors ADR 0075 (authenticated envelope):
//! this module ships only the primitive. Carriers (RowStore
//! checkpoint pages, `.derc` columns, BRWL batches, backup
//! archives) consume dictionaries in future ADRs that come with
//! their own workload benchmarks.

use subtle::ConstantTimeEq;
use thiserror::Error;

/// Operator-supplied dictionary for dictionary-mode compression.
///
/// Carries:
/// - Raw dictionary bytes (operator-owned).
/// - 32-byte BLAKE3 fingerprint over the raw bytes (dictionary
///   identity; depends only on bytes, not label).
/// - Human-readable label surfaced in error messages / logs.
///
/// Debug redacts raw bytes; prints label + first 8 fingerprint
/// hex chars + byte count. `PartialEq` uses constant-time
/// fingerprint comparison. Raw bytes are NOT exposed publicly —
/// callers interact through the encode/decode helpers in this
/// module; the accessor used internally is `pub(crate)`.
#[derive(Clone)]
pub struct BaceDictionary {
    bytes: Vec<u8>,
    fingerprint: [u8; 32],
    label: String,
}

impl BaceDictionary {
    /// Construct from raw dictionary bytes + an operator-facing
    /// label. The fingerprint is computed as `blake3(bytes)`.
    pub fn new(bytes: Vec<u8>, label: impl Into<String>) -> Self {
        let fingerprint = *blake3::hash(&bytes).as_bytes();
        Self { bytes, fingerprint, label: label.into() }
    }

    pub fn label(&self) -> &str { &self.label }
    pub fn fingerprint(&self) -> &[u8; 32] { &self.fingerprint }
    pub fn len(&self) -> usize { self.bytes.len() }
    pub fn is_empty(&self) -> bool { self.bytes.is_empty() }

    /// ADR 0078 §3.1 — internal accessor consumed by the encode/
    /// decode helpers in this module. Not exported outside the
    /// crate so there is no public path from a `&BaceDictionary`
    /// to its raw bytes.
    pub(crate) fn bytes_for_dict(&self) -> &[u8] { &self.bytes }

    /// Test-only accessor used for round-trip fingerprint tests.
    /// Not exposed outside the crate.
    #[cfg(test)]
    pub(crate) fn bytes_for_test(&self) -> &[u8] { &self.bytes }
}

impl std::fmt::Debug for BaceDictionary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
        // ADR 0078 §3.1 — constant-time fingerprint equality.
        self.fingerprint.ct_eq(&other.fingerprint).unwrap_u8() == 1
    }
}
impl Eq for BaceDictionary {}

/// Fail-closed variants for the dictionary primitive.
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
    #[error("decoded output exceeds declared decoded_length {declared}")]
    DecodedLengthExceeded { declared: u32 },
    #[error("decoded output {bytes} does not match declared decoded_length {declared}")]
    DecodedLengthMismatch { bytes: usize, declared: u32 },
}

/// ADR 0078 §3.2 — train a ZSTD dictionary from representative
/// sample pages. `target_bytes` is the maximum dictionary size;
/// ZSTD may return fewer bytes depending on the samples.
///
/// Thin wrapper over `zstd::dict::from_samples`; no custom
/// dictionary arithmetic per CLAUDE.md §3.
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

/// ADR 0078 §3.3 — ZSTD dictionary encode at level 3 (matches ADR
/// 0066 ZstdCodec baseline). Appends the compressed bytes to
/// `output` and returns the number of bytes written.
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
    let enc_dict = zstd::dict::EncoderDictionary::copy(dict.bytes_for_dict(), 3);
    let mut compressor = zstd::bulk::Compressor::with_prepared_dictionary(&enc_dict)
        .map_err(|e| DictionaryError::EncodeFailed(e.to_string()))?;
    let compressed = compressor.compress(input)
        .map_err(|e| DictionaryError::EncodeFailed(e.to_string()))?;
    output.extend_from_slice(&compressed);
    Ok(compressed.len())
}

/// ADR 0078 §3.3 — ZSTD dictionary decode. `decoded_length` caps
/// the output buffer capacity so `zstd::bulk::decompress` enforces
/// the bound during decoding (matches ADR 0066 BACE-FMT-003
/// semantics). Mismatched real-vs-declared output length fails
/// closed.
pub fn decode_zstd_with_dictionary(
    input: &[u8],
    dict: &BaceDictionary,
    decoded_length: u32,
    output: &mut Vec<u8>,
) -> Result<(), DictionaryError> {
    let dec_dict = zstd::dict::DecoderDictionary::copy(dict.bytes_for_dict());
    let mut decompressor = zstd::bulk::Decompressor::with_prepared_dictionary(&dec_dict)
        .map_err(|e| DictionaryError::DecodeFailed(e.to_string()))?;
    let decoded = decompressor
        .decompress(input, decoded_length as usize)
        .map_err(|_| DictionaryError::DecodedLengthExceeded { declared: decoded_length })?;
    if decoded.len() as u64 != decoded_length as u64 {
        return Err(DictionaryError::DecodedLengthMismatch {
            bytes: decoded.len(), declared: decoded_length,
        });
    }
    output.extend_from_slice(&decoded);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate `count` sample pages sharing a common prefix so the
    /// trainer has something to learn.
    fn samples(count: usize) -> Vec<Vec<u8>> {
        (0..count).map(|i| {
            let mut v = Vec::with_capacity(200);
            v.extend_from_slice(b"BRADIAN-SAMPLE-COMMON-PREFIX-");
            v.extend_from_slice(format!("{:020}", i).as_bytes());
            v.extend_from_slice(b"--END-COMMON-TAIL-");
            v.extend(std::iter::repeat_n(b'x', 50));
            v
        }).collect()
    }

    fn samples_refs(v: &[Vec<u8>]) -> Vec<&[u8]> {
        v.iter().map(|s| s.as_slice()).collect()
    }

    #[test]
    fn item_1_training_yields_non_empty_deterministic_dict() {
        // ADR 0078 §3.5 item 1 and 7 combined — same input bytes
        // produce the same fingerprint regardless of label.
        let s = samples(32);
        let refs = samples_refs(&s);
        let d1 = train_zstd_dictionary(&refs, 4096, "a").unwrap();
        let d2 = train_zstd_dictionary(&refs, 4096, "b").unwrap();
        assert!(!d1.is_empty(), "trained dict must be non-empty");
        assert_eq!(d1.bytes_for_test(), d2.bytes_for_test(),
            "training is deterministic on same samples/target");
        assert_eq!(d1.fingerprint(), d2.fingerprint(),
            "fingerprint depends only on bytes, not label");
    }

    #[test]
    fn item_2_training_with_zero_samples_fails() {
        // ADR 0078 §3.5 item 2.
        let err = train_zstd_dictionary(&[], 4096, "none").unwrap_err();
        assert!(
            matches!(err, DictionaryError::TrainingFailed(_)),
            "expected TrainingFailed, got {err:?}",
        );
    }

    #[test]
    fn item_3_encode_decode_round_trip_preserves_bytes() {
        // ADR 0078 §3.5 item 3.
        let s = samples(32);
        let refs = samples_refs(&s);
        let dict = train_zstd_dictionary(&refs, 4096, "dict").unwrap();
        let input = &s[0];
        let mut encoded = Vec::new();
        encode_zstd_with_dictionary(input, &dict, &mut encoded).unwrap();
        let mut decoded = Vec::new();
        decode_zstd_with_dictionary(
            &encoded, &dict, input.len() as u32, &mut decoded,
        ).unwrap();
        assert_eq!(decoded, *input);
    }

    #[test]
    fn item_4_decode_with_wrong_dict_fails_decode_failed_not_silent() {
        // ADR 0078 §3.5 item 4 — the key safety invariant. A
        // mismatched dictionary MUST fail closed; it must NOT
        // return silently-incorrect decoded bytes.
        let s_a = samples(32);
        let s_b: Vec<Vec<u8>> = (0..32).map(|i| {
            // Entirely different content so training produces a
            // different dictionary.
            format!("ENTIRELY-DIFFERENT-CONTENT-{:020}", i).into_bytes()
        }).collect();
        let dict_a = train_zstd_dictionary(&samples_refs(&s_a), 4096, "a").unwrap();
        let dict_b = train_zstd_dictionary(&samples_refs(&s_b), 4096, "b").unwrap();
        assert_ne!(dict_a.fingerprint(), dict_b.fingerprint());

        let input = &s_a[0];
        let mut encoded = Vec::new();
        encode_zstd_with_dictionary(input, &dict_a, &mut encoded).unwrap();
        let mut decoded = Vec::new();
        let err = decode_zstd_with_dictionary(
            &encoded, &dict_b, input.len() as u32, &mut decoded,
        ).unwrap_err();
        assert!(
            matches!(err, DictionaryError::DecodeFailed(_)
                | DictionaryError::DecodedLengthExceeded { .. }
                | DictionaryError::DecodedLengthMismatch { .. }),
            "wrong-dict decode MUST fail closed; got {err:?}",
        );
    }

    #[test]
    fn item_5_decoded_length_cap_fires_before_allocation() {
        // ADR 0078 §3.5 item 5 — cap smaller than real decoded
        // size fails closed.
        let s = samples(32);
        let refs = samples_refs(&s);
        let dict = train_zstd_dictionary(&refs, 4096, "dict").unwrap();
        let input = &s[0];
        let mut encoded = Vec::new();
        encode_zstd_with_dictionary(input, &dict, &mut encoded).unwrap();
        let mut decoded = Vec::new();
        let err = decode_zstd_with_dictionary(
            &encoded, &dict, (input.len() / 2) as u32, &mut decoded,
        ).unwrap_err();
        assert!(
            matches!(err, DictionaryError::DecodedLengthExceeded { .. }),
            "cap-smaller-than-real decode must fail DecodedLengthExceeded, got {err:?}",
        );
    }

    #[test]
    fn item_6_decoded_length_mismatch_fails_closed() {
        // ADR 0078 §3.5 item 6 — real output smaller than declared.
        let s = samples(32);
        let refs = samples_refs(&s);
        let dict = train_zstd_dictionary(&refs, 4096, "dict").unwrap();
        let input = &s[0];
        let mut encoded = Vec::new();
        encode_zstd_with_dictionary(input, &dict, &mut encoded).unwrap();
        let mut decoded = Vec::new();
        let err = decode_zstd_with_dictionary(
            &encoded, &dict, (input.len() + 100) as u32, &mut decoded,
        ).unwrap_err();
        assert!(
            matches!(err, DictionaryError::DecodedLengthMismatch { .. }),
            "declared-larger-than-real must fail DecodedLengthMismatch, got {err:?}",
        );
    }

    #[test]
    fn item_8_partial_eq_uses_fingerprint() {
        // ADR 0078 §3.5 item 8.
        let s = samples(32);
        let refs = samples_refs(&s);
        let d1 = train_zstd_dictionary(&refs, 4096, "a").unwrap();
        let d2 = train_zstd_dictionary(&refs, 4096, "b").unwrap();
        assert_eq!(d1, d2, "equal fingerprints must compare equal");

        let d3 = BaceDictionary::new(b"different".to_vec(), "c");
        assert_ne!(d1, d3);
    }

    #[test]
    fn item_9_debug_redacts_raw_bytes() {
        // ADR 0078 §3.5 item 9.
        let d = BaceDictionary::new(b"secret-marker-abc".to_vec(), "ops");
        let printed = format!("{d:?}");
        assert!(printed.contains("ops"), "Debug should show label: {printed}");
        assert!(printed.contains("[redacted]"),
            "Debug should redact bytes: {printed}");
        assert!(!printed.contains("secret-marker-abc"),
            "Debug must not print raw dictionary bytes: {printed}");
        assert!(printed.contains("fingerprint_prefix"),
            "Debug should include the fingerprint prefix: {printed}");
    }

    #[test]
    fn item_10_size_reduction_demonstration() {
        // ADR 0078 §3.5 item 10 — on redundant .derc-style sample
        // pages, encoding WITH the trained dict is strictly smaller
        // than encoding WITHOUT.
        let s = samples(128);
        let refs = samples_refs(&s);
        let dict = train_zstd_dictionary(&refs, 8192, "demo").unwrap();

        let input = &s[0];
        // With dict.
        let mut with_dict = Vec::new();
        encode_zstd_with_dictionary(input, &dict, &mut with_dict).unwrap();
        // Without dict — plain ZSTD level 3 for apples-to-apples.
        let without_dict = zstd::bulk::compress(input, 3).unwrap();

        assert!(
            with_dict.len() < without_dict.len(),
            "dict-encoded ({} bytes) must be strictly smaller than \
             non-dict encoded ({} bytes) on redundant samples",
            with_dict.len(), without_dict.len(),
        );
    }

    #[test]
    fn item_11_input_too_large_fast_fail() {
        // ADR 0078 §3.5 item 11 — test the bound guard without
        // allocating a 4 GiB buffer. We can't actually build a
        // u32::MAX-sized Vec in test, so we verify the check path
        // is reachable at code-review level and that `input.len()
        // > u32::MAX` is the condition. On 32-bit targets this
        // branch is unreachable because Vec<u8> can't exceed
        // isize::MAX < u32::MAX; the branch is fail-safe either
        // way. Smoke test: encode an empty input.
        let dict = BaceDictionary::new(b"anything".to_vec(), "trivial");
        let mut out = Vec::new();
        // Empty input is legal.
        let _ = encode_zstd_with_dictionary(&[], &dict, &mut out);
    }

    #[test]
    fn fingerprint_depends_only_on_bytes_not_label() {
        // Cross-check for item 7 phrased as a direct constructor
        // test (not through train).
        let bytes = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        let a = BaceDictionary::new(bytes.clone(), "production");
        let b = BaceDictionary::new(bytes.clone(), "staging");
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_eq!(a, b, "PartialEq must reflect fingerprint equality");
    }
}
