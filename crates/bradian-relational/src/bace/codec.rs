//! BACE codec types and the RAW codec (accepted ADR 0061).

use thiserror::Error;

/// Stable codec identifier. The `u16` namespace is partitioned per
/// ADR 0061 §3.1. IDs 0–5 FROZEN as `.brcs` v1 legacy mirror. IDs 6–15
/// legacy compat buffer. 16–31 core inner, 32–47 specialized, 48–63
/// outer stream compression, 64–255 reserved BACE future, 256+
/// vendor/experimental.
///
/// The first implementation allocates exactly one ID — `RAW = 48` in
/// the outer-compression range (see `raw::RAW_ID`). Inner-encoding
/// IDs 16–47 remain unassigned until explicit owner signoff.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CodecId(pub u16);

impl CodecId {
    pub const fn raw() -> Self { raw::RAW_ID }
}

/// Determinism class per ADR 0061 §3.3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Determinism {
    /// Same input bytes always produce the same output bytes on every
    /// supported architecture and SIMD path. BACE-NFR-PORT-001 obligation.
    Deterministic,
    /// Output bytes may differ across SIMD paths but visible SQL results
    /// do not. Allowed only for codecs whose cross-build equivalence is
    /// proven by a golden corpus on each supported target.
    HardwareDependent,
}

/// Capability flags per ADR 0061 §3.3. Explicit struct instead of a
/// bitflags crate so the project dependency set stays unchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapabilityFlags {
    /// If a reader cannot resolve this codec ID, the operation fails
    /// closed. Matches the envelope's `flags.critical` bit.
    pub critical: bool,
    /// Not selected for durable writes unless the caller explicitly
    /// opts in. Reads of previously-written experimental payloads are
    /// always honored (ADR 0061 §5 D2).
    pub experimental: bool,
    /// Codec preserves the distinction between SQL NULL and non-NULL.
    /// Default `true` for every shipped BACE codec.
    pub preserves_null_distinct: bool,
    /// Codec may be chained under an outer stream compression. Default
    /// `true` for inner encodings; `false` for outer codecs.
    pub allows_outer_compression: bool,
}

impl CapabilityFlags {
    pub const fn default_inner() -> Self {
        Self { critical: false, experimental: false, preserves_null_distinct: true, allows_outer_compression: true }
    }
    pub const fn default_outer() -> Self {
        Self { critical: false, experimental: false, preserves_null_distinct: true, allows_outer_compression: false }
    }
}

/// Immutable capability descriptor published by every registered codec.
/// Matches ADR 0061 §3.3.
#[derive(Clone, Debug)]
pub struct CodecCapability {
    pub id: CodecId,
    pub format_version: u16,
    pub max_input_bytes: u32,
    pub max_output_bytes: u32,
    pub determinism: Determinism,
    pub flags: CapabilityFlags,
    /// Human-readable name, for logs only. Not part of the on-disk
    /// format.
    pub name: &'static str,
}

/// Errors from a codec's encode path.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EncodeError {
    #[error("input {bytes} exceeds codec max_input_bytes {limit}")]
    InputTooLarge { bytes: usize, limit: u32 },
    #[error("encoder would produce {bytes} bytes, exceeding codec max_output_bytes {limit}")]
    OutputTooLarge { bytes: usize, limit: u32 },
    #[error("codec {id:?} not registered")]
    UnknownCodec { id: CodecId },
}

/// Errors from a codec's decode path.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecodeError {
    #[error("codec {id:?} not registered")]
    UnknownCodec { id: CodecId },
    #[error("codec {id:?} declared format_version {declared}, registry max supported {max_supported}")]
    UnsupportedFormatVersion { id: CodecId, declared: u16, max_supported: u16 },
    #[error("decoded output {bytes} exceeds declared decoded_length {declared}")]
    DecodedLengthExceeded { bytes: usize, declared: u32 },
    #[error("decoded output {bytes} does not match declared decoded_length {declared}")]
    DecodedLengthMismatch { bytes: usize, declared: u32 },
    #[error("declared encoded_length does not match actual input bytes")]
    EncodedLengthMismatch,
}

/// The codec trait. A conformant implementation MUST produce output
/// that round-trips through decode, MUST never produce more than
/// `decoded_length` bytes during decode (ADR 0062 §4 step 13), and
/// MUST satisfy its declared `Determinism` class.
pub trait Codec: Send + Sync {
    fn capability(&self) -> &CodecCapability;

    /// Encode `input` into `output`. Returns the number of bytes written.
    /// Must respect `capability().max_output_bytes`.
    fn encode(&self, input: &[u8], output: &mut Vec<u8>) -> Result<usize, EncodeError>;

    /// Decode `input` into `output`. Must respect `decoded_length` as
    /// a hard upper bound DURING decoding, not merely after.
    fn decode(&self, input: &[u8], decoded_length: u32, output: &mut Vec<u8>) -> Result<(), DecodeError>;
}

pub mod raw {
    //! The RAW codec. ID 48. Deterministic by definition. Output bytes
    //! equal input bytes; no transformation applied.
    //!
    //! This is the first codec allocated under ADR 0061. It sits in the
    //! outer-compression range (48–63) which has no legacy constraint
    //! and no interference with any future inner-encoding ID assignment.

    use super::{CapabilityFlags, Codec, CodecCapability, CodecId, Determinism, DecodeError, EncodeError};

    pub const RAW_ID: CodecId = CodecId(48);

    const CAPABILITY: CodecCapability = CodecCapability {
        id: RAW_ID,
        format_version: 1,
        max_input_bytes: u32::MAX,
        max_output_bytes: u32::MAX,
        determinism: Determinism::Deterministic,
        flags: CapabilityFlags {
            critical: true,       // RAW must always be available; it is the fallback
            experimental: false,
            preserves_null_distinct: true,
            allows_outer_compression: false,
        },
        name: "RAW",
    };

    #[derive(Debug, Default)]
    pub struct RawCodec;

    impl Codec for RawCodec {
        fn capability(&self) -> &CodecCapability { &CAPABILITY }

        fn encode(&self, input: &[u8], output: &mut Vec<u8>) -> Result<usize, EncodeError> {
            if input.len() > CAPABILITY.max_input_bytes as usize {
                return Err(EncodeError::InputTooLarge {
                    bytes: input.len(),
                    limit: CAPABILITY.max_input_bytes,
                });
            }
            output.extend_from_slice(input);
            Ok(input.len())
        }

        fn decode(&self, input: &[u8], decoded_length: u32, output: &mut Vec<u8>) -> Result<(), DecodeError> {
            if input.len() as u64 > decoded_length as u64 {
                return Err(DecodeError::DecodedLengthExceeded {
                    bytes: input.len(),
                    declared: decoded_length,
                });
            }
            if input.len() as u64 != decoded_length as u64 {
                return Err(DecodeError::DecodedLengthMismatch {
                    bytes: input.len(),
                    declared: decoded_length,
                });
            }
            output.extend_from_slice(input);
            Ok(())
        }
    }
}

pub mod zstd {
    //! The ZSTD codec. ID 49. HardwareDependent per ADR 0066 §3.2 and
    //! ADR 0061 §5 item 4.
    //!
    //! Uses the ZSTD **frame format** (not raw blocks) at compression
    //! level 3 — identical to the `.brcs` v1 default wrap. Decoder
    //! enforces the envelope-declared `decoded_length` as the output
    //! capacity bound at the library call, satisfying BACE-FMT-003
    //! "bounded output DURING decoding, not merely after."
    //!
    //! Not in the default `PlatformBaseline::allowed_codecs`; operators
    //! opt in by passing `allowed_codecs = [ZSTD_ID, RAW_ID]` on the
    //! wiring they hand to `RowStore::open_with_bace`.

    use super::{CapabilityFlags, Codec, CodecCapability, CodecId, DecodeError, Determinism, EncodeError};

    pub const ZSTD_ID: CodecId = CodecId(49);
    pub const ZSTD_FORMAT_VERSION: u16 = 1;
    /// Pinned level — matches ZSTD_CLEVEL_DEFAULT and the `.brcs` default.
    /// A future ADR may expose this via `compression.zstd_level`.
    pub const ZSTD_LEVEL: i32 = 3;

    const CAPABILITY: CodecCapability = CodecCapability {
        id: ZSTD_ID,
        format_version: ZSTD_FORMAT_VERSION,
        max_input_bytes: u32::MAX,
        max_output_bytes: u32::MAX,
        determinism: Determinism::HardwareDependent,
        flags: CapabilityFlags {
            critical: false,                 // RAW stays the critical fallback
            experimental: false,
            preserves_null_distinct: true,
            allows_outer_compression: false, // ZSTD IS an outer compressor
        },
        name: "ZSTD",
    };

    #[derive(Debug, Default)]
    pub struct ZstdCodec;

    impl Codec for ZstdCodec {
        fn capability(&self) -> &CodecCapability { &CAPABILITY }

        fn encode(&self, input: &[u8], output: &mut Vec<u8>) -> Result<usize, EncodeError> {
            if input.len() > CAPABILITY.max_input_bytes as usize {
                return Err(EncodeError::InputTooLarge {
                    bytes: input.len(),
                    limit: CAPABILITY.max_input_bytes,
                });
            }
            let compressed = ::zstd::bulk::compress(input, ZSTD_LEVEL).map_err(|_| {
                EncodeError::OutputTooLarge { bytes: input.len(), limit: CAPABILITY.max_output_bytes }
            })?;
            if compressed.len() > CAPABILITY.max_output_bytes as usize {
                return Err(EncodeError::OutputTooLarge {
                    bytes: compressed.len(),
                    limit: CAPABILITY.max_output_bytes,
                });
            }
            output.extend_from_slice(&compressed);
            Ok(compressed.len())
        }

        fn decode(&self, input: &[u8], decoded_length: u32, output: &mut Vec<u8>) -> Result<(), DecodeError> {
            // Pass decoded_length as the output capacity to zstd::bulk::decompress.
            // The library itself enforces the bound DURING decoding — if the
            // frame would produce more bytes, decompress fails rather than
            // over-allocating or over-writing. Matches BACE-FMT-003.
            let decoded = ::zstd::bulk::decompress(input, decoded_length as usize).map_err(|_| {
                DecodeError::DecodedLengthExceeded { bytes: usize::MAX, declared: decoded_length }
            })?;
            if decoded.len() as u64 != decoded_length as u64 {
                return Err(DecodeError::DecodedLengthMismatch {
                    bytes: decoded.len(),
                    declared: decoded_length,
                });
            }
            output.extend_from_slice(&decoded);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::raw::RawCodec;
    use super::zstd::{ZstdCodec, ZSTD_FORMAT_VERSION, ZSTD_ID, ZSTD_LEVEL};

    #[test]
    fn raw_codec_identifies_itself_as_id_48_deterministic_critical() {
        let cap = RawCodec.capability();
        assert_eq!(cap.id, CodecId(48));
        assert_eq!(cap.format_version, 1);
        assert_eq!(cap.determinism, Determinism::Deterministic);
        assert!(cap.flags.critical);
        assert!(!cap.flags.experimental);
    }

    #[test]
    fn raw_codec_round_trip_preserves_every_byte_exactly() {
        let codec = RawCodec;
        for bytes in [b"" as &[_], b"a", b"abc", &[0xff; 4096], &[0; 1]] {
            let mut encoded = Vec::new();
            codec.encode(bytes, &mut encoded).unwrap();
            assert_eq!(encoded.as_slice(), bytes, "encode must be identity");
            let mut decoded = Vec::new();
            codec.decode(&encoded, bytes.len() as u32, &mut decoded).unwrap();
            assert_eq!(decoded.as_slice(), bytes, "round trip must preserve bytes");
        }
    }

    #[test]
    fn raw_decoder_rejects_declared_length_mismatch() {
        let codec = RawCodec;
        let mut out = Vec::new();
        let err = codec.decode(&[1, 2, 3], 5, &mut out).unwrap_err();
        assert!(matches!(err, DecodeError::DecodedLengthMismatch { bytes: 3, declared: 5 }));
    }

    #[test]
    fn raw_decoder_rejects_input_exceeding_declared_decoded_length() {
        let codec = RawCodec;
        let mut out = Vec::new();
        let err = codec.decode(&[1, 2, 3, 4, 5], 3, &mut out).unwrap_err();
        assert!(matches!(err, DecodeError::DecodedLengthExceeded { bytes: 5, declared: 3 }));
    }

    // ---- ZSTD codec tests (ADR 0066) ----

    #[test]
    fn zstd_codec_identifies_itself_as_id_49_hardware_dependent_non_critical_outer() {
        let cap = ZstdCodec.capability();
        assert_eq!(cap.id, ZSTD_ID);
        assert_eq!(cap.id.0, 49);
        assert_eq!(cap.format_version, ZSTD_FORMAT_VERSION);
        assert_eq!(cap.format_version, 1);
        assert_eq!(cap.determinism, Determinism::HardwareDependent);
        assert!(!cap.flags.critical, "RAW stays the critical fallback");
        assert!(!cap.flags.allows_outer_compression, "ZSTD IS an outer compressor");
        assert!(cap.flags.preserves_null_distinct);
        assert!(!cap.flags.experimental);
    }

    #[test]
    fn zstd_pinned_level_matches_brcs_default() {
        // ADR 0066 §3.1 — level 3 matches ZSTD_CLEVEL_DEFAULT and the
        // .brcs v1 default wrap. If this ever changes, a new ADR is
        // required (the on-disk bytes depend on it).
        assert_eq!(ZSTD_LEVEL, 3);
    }

    #[test]
    fn zstd_round_trip_empty_input_golden_corpus() {
        // ADR 0066 §3.6 — "zstd_empty_input".
        let codec = ZstdCodec;
        let input: &[u8] = b"";
        let mut encoded = Vec::new();
        codec.encode(input, &mut encoded).unwrap();
        assert!(encoded.len() >= 9, "ZSTD frame overhead is at least 9 bytes");
        let mut decoded = Vec::new();
        codec.decode(&encoded, input.len() as u32, &mut decoded).unwrap();
        assert_eq!(decoded.as_slice(), input);
    }

    #[test]
    fn zstd_round_trip_short_incompressible_golden_corpus() {
        // ADR 0066 §3.6 — "zstd_short_incompressible". Short input that
        // cannot compress; we don't assert size, only round-trip.
        let codec = ZstdCodec;
        let input: &[u8] = b"abcdef";
        let mut encoded = Vec::new();
        codec.encode(input, &mut encoded).unwrap();
        let mut decoded = Vec::new();
        codec.decode(&encoded, input.len() as u32, &mut decoded).unwrap();
        assert_eq!(decoded.as_slice(), input);
    }

    #[test]
    fn zstd_round_trip_long_repetitive_golden_corpus_actually_saves_space() {
        // ADR 0066 §3.6 — "zstd_long_repetitive". Compression MUST save
        // space on highly redundant input; this is the core BACE-GOAL-001
        // demonstration that ZSTD is worth shipping.
        let codec = ZstdCodec;
        let input = vec![b'x'; 4_000];
        let mut encoded = Vec::new();
        codec.encode(&input, &mut encoded).unwrap();
        assert!(
            encoded.len() < 100,
            "4 000 copies of 'x' must compress to under 100 bytes, got {}",
            encoded.len()
        );
        let mut decoded = Vec::new();
        codec.decode(&encoded, input.len() as u32, &mut decoded).unwrap();
        assert_eq!(decoded, input);
    }

    #[test]
    fn zstd_decoder_fails_closed_when_declared_decoded_length_is_too_small() {
        // The decoded_length argument is passed to zstd::bulk::decompress
        // as the output capacity. If it's smaller than the frame's real
        // decoded size, the library refuses to over-allocate and the
        // decode call fails. Satisfies ADR 0066 §3.4 and BACE-FMT-003.
        let codec = ZstdCodec;
        let input = vec![b'x'; 4_000];
        let mut encoded = Vec::new();
        codec.encode(&input, &mut encoded).unwrap();
        let mut decoded = Vec::new();
        let err = codec.decode(&encoded, 100, &mut decoded).unwrap_err();
        assert!(
            matches!(err, DecodeError::DecodedLengthExceeded { .. }),
            "expected DecodedLengthExceeded, got {:?}",
            err
        );
    }

    #[test]
    fn zstd_decoder_fails_closed_when_declared_decoded_length_disagrees_with_frame() {
        // Compress 100 bytes and declare decoded_length=200. Decompression
        // succeeds within the 200-byte cap but produces only 100 bytes;
        // the mismatch check must fire.
        let codec = ZstdCodec;
        let input = vec![b'x'; 100];
        let mut encoded = Vec::new();
        codec.encode(&input, &mut encoded).unwrap();
        let mut decoded = Vec::new();
        let err = codec.decode(&encoded, 200, &mut decoded).unwrap_err();
        assert!(
            matches!(err, DecodeError::DecodedLengthMismatch { bytes: 100, declared: 200 }),
            "expected DecodedLengthMismatch 100/200, got {:?}",
            err
        );
    }
}
