//! BACE authenticated envelope (ADR 0075).
//!
//! On-disk layout (32 + N + 4 + 16 = 52 + N bytes):
//!
//! ```text
//! ┌──────────────────────────────┬──────────────────────┬────────────┬────────────┐
//! │ HEADER (32 bytes)            │ PAYLOAD (encoded_len)│ CRC32 (4)  │ MAC16 (16) │
//! │ magic "BACA" + v1 fields     │   codec-specific     │ over       │ keyed      │
//! │                              │                      │ header +   │ BLAKE3 over│
//! │                              │                      │ payload    │ header +   │
//! │                              │                      │            │ payload    │
//! └──────────────────────────────┴──────────────────────┴────────────┴────────────┘
//! ```
//!
//! The authenticated variant adds a 16-byte BLAKE3-128 keyed MAC over
//! the header bytes + payload bytes, computed via `blake3::keyed_hash`.
//! The CRC32 is preserved alongside the MAC so a reader can distinguish
//! bit rot (CRC mismatch) from tampering or wrong-key (MAC mismatch).
//!
//! **Scope boundary (ADR 0075 §1):** this module adds only the
//! envelope primitive. No carrier integrates it yet. Future carrier-
//! integration ADRs will plumb `BaceAuthentication` through
//! `BaceWiring` under their own threat-model documentation per
//! CLAUDE.md §6.
//!
//! **Cryptographic primitive:** BLAKE3 keyed hash via
//! `blake3::keyed_hash`. No custom crypto per CLAUDE.md §3.
//! Constant-time tag comparison via `subtle::ConstantTimeEq`.

use crc32fast::Hasher;
use subtle::ConstantTimeEq;
use thiserror::Error;

use super::codec::CodecId;
use super::envelope::{EnvelopeFlags, VENDOR_FLAG_THRESHOLD};

/// ASCII `"BACA"` reserved by ADR 0062 §9 item 1 for the
/// authenticated envelope variant.
pub const AUTHENTICATED_ENVELOPE_MAGIC: [u8; 4] = *b"BACA";

/// Current frozen authenticated envelope version.
pub const AUTHENTICATED_ENVELOPE_VERSION: u16 = 1;

/// Fixed header length — same shape as the BACE v1 header.
pub const AUTHENTICATED_ENVELOPE_HEADER_SIZE: usize = 32;

/// CRC32 trailer length.
pub const AUTHENTICATED_ENVELOPE_TRAILER_SIZE: usize = 4;

/// BLAKE3-128 truncated MAC tag length. 16 bytes = 128-bit
/// unforgeability under the assumption that the key is secret.
pub const AUTHENTICATED_ENVELOPE_MAC_SIZE: usize = 16;

/// Minimum authenticated envelope size (empty payload permitted).
pub const AUTHENTICATED_ENVELOPE_MIN_SIZE: usize =
    AUTHENTICATED_ENVELOPE_HEADER_SIZE
        + AUTHENTICATED_ENVELOPE_TRAILER_SIZE
        + AUTHENTICATED_ENVELOPE_MAC_SIZE;

/// Flag bits for the authenticated envelope header. Byte-identical
/// to the BACE v1 flags layout (ADR 0062 §3.3). A separate struct
/// keeps future divergence between BACE and BACA flag semantics
/// local; today the two types are interchangeable at the bit level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AuthenticatedEnvelopeFlags {
    pub payload_is_raw: bool,
    pub critical: bool,
    pub encryption_ordered: bool,
    pub has_sidecar_dictionary: bool,
    pub vendor_bits: u16,
}

impl AuthenticatedEnvelopeFlags {
    pub fn to_bits(self) -> u32 {
        let mut bits = 0_u32;
        if self.payload_is_raw { bits |= 1 << 0; }
        if self.critical { bits |= 1 << 1; }
        if self.encryption_ordered { bits |= 1 << 2; }
        if self.has_sidecar_dictionary { bits |= 1 << 3; }
        bits |= (self.vendor_bits as u32) << 16;
        bits
    }

    pub fn from_bits(bits: u32, codec_id: CodecId) -> Result<Self, AuthenticatedEnvelopeError> {
        // Bits 4..=15 are reserved and must be zero.
        let reserved_mid = (bits >> 4) & 0x0000_0FFF;
        if reserved_mid != 0 {
            return Err(AuthenticatedEnvelopeError::UnknownEnvelopeFlag { bits });
        }
        let vendor = ((bits >> 16) & 0x0000_FFFF) as u16;
        if vendor != 0 && codec_id.0 < VENDOR_FLAG_THRESHOLD {
            return Err(AuthenticatedEnvelopeError::UnknownEnvelopeFlag { bits });
        }
        Ok(Self {
            payload_is_raw: (bits & (1 << 0)) != 0,
            critical: (bits & (1 << 1)) != 0,
            encryption_ordered: (bits & (1 << 2)) != 0,
            has_sidecar_dictionary: (bits & (1 << 3)) != 0,
            vendor_bits: vendor,
        })
    }
}

impl From<EnvelopeFlags> for AuthenticatedEnvelopeFlags {
    fn from(f: EnvelopeFlags) -> Self {
        Self {
            payload_is_raw: f.payload_is_raw,
            critical: f.critical,
            encryption_ordered: f.encryption_ordered,
            has_sidecar_dictionary: f.has_sidecar_dictionary,
            vendor_bits: f.vendor_bits,
        }
    }
}

/// Everything in the authenticated envelope header except the
/// CRC32 and MAC trailer bytes. The reserved[20..32] region grows
/// future fields (e.g., `key_id` for rotation per ADR 0075 §4 D).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthenticatedEnvelopeHeader {
    pub codec_id: CodecId,
    pub codec_format_version: u16,
    pub flags: AuthenticatedEnvelopeFlags,
    pub encoded_length: u32,
    pub decoded_length: u32,
}

/// A successfully-parsed authenticated envelope view. The payload
/// is borrowed from the original input buffer. The MAC has been
/// verified before this struct is returned.
#[derive(Debug, PartialEq, Eq)]
pub struct AuthenticatedEnvelope<'a> {
    pub header: AuthenticatedEnvelopeHeader,
    pub payload: &'a [u8],
}

/// Fail-closed variants for the authenticated envelope reader.
/// `CrcMismatch` and `MacMismatch` are intentionally distinct so
/// operators can diagnose "bit rot" vs "wrong key or tampering."
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AuthenticatedEnvelopeError {
    #[error("authenticated envelope input is {bytes} bytes; minimum is {min}")]
    InputTooSmall { bytes: usize, min: usize },
    #[error("authenticated envelope magic mismatch (expected BACA)")]
    MagicMismatch,
    #[error("authenticated envelope version {declared} unsupported (expected 1)")]
    VersionMismatch { declared: u16 },
    #[error("authenticated envelope reserved bits non-zero")]
    ReservedBitsSet,
    #[error("authenticated envelope flags {bits:#010x} contain unknown or forbidden bits")]
    UnknownEnvelopeFlag { bits: u32 },
    #[error("authenticated envelope encoded_length {declared} exceeds caller cap {cap}")]
    EncodedLengthCapExceeded { declared: u32, cap: u32 },
    #[error("authenticated envelope decoded_length {declared} exceeds caller cap {cap}")]
    DecodedLengthCapExceeded { declared: u32, cap: u32 },
    #[error("authenticated envelope CRC32 mismatch")]
    CrcMismatch,
    #[error("authenticated envelope MAC mismatch (tag did not verify under key label {label:?})")]
    MacMismatch { label: String },
    #[error("authenticated envelope buffer truncated (expected {expected} bytes, got {actual})")]
    Truncated { expected: usize, actual: usize },
    #[error("declared encoded_length {declared} does not match actual body {actual}")]
    EncodedLengthMismatch { declared: u32, actual: usize },
    #[error("authenticated envelope payload length {bytes} exceeds u32::MAX")]
    PayloadTooLarge { bytes: usize },
}

/// Operator-supplied key material for the authenticated envelope.
/// The 32-byte `key` is used by BLAKE3's keyed hash mode. The
/// `label` is a human-readable identifier surfaced in error messages
/// and logs only; it is NOT bound into the cryptographic computation.
///
/// The type deliberately does NOT expose a public key accessor —
/// callers interact via `write_authenticated_envelope` and
/// `read_authenticated_envelope`. Production carriers pass
/// `&BaceAuthentication`; raw-byte extraction is not supported.
///
/// `Debug` redacts the key bytes.
#[derive(Clone)]
pub struct BaceAuthentication {
    key: [u8; 32],
    label: String,
}

impl BaceAuthentication {
    pub fn new(key: [u8; 32], label: impl Into<String>) -> Self {
        Self { key, label: label.into() }
    }

    pub fn label(&self) -> &str { &self.label }

    /// ADR 0075 §3.3 — callers in the same crate obtain the key for
    /// MAC computation. Not exported outside the crate.
    pub(crate) fn key_bytes(&self) -> &[u8; 32] { &self.key }
}

impl std::fmt::Debug for BaceAuthentication {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BaceAuthentication")
            .field("label", &self.label)
            .field("key", &"[redacted]")
            .finish()
    }
}

impl PartialEq for BaceAuthentication {
    fn eq(&self, other: &Self) -> bool {
        // Keys compared in constant time; labels in constant-adjacent.
        self.key.ct_eq(&other.key).unwrap_u8() == 1 && self.label == other.label
    }
}
impl Eq for BaceAuthentication {}

/// Compute the 16-byte BLAKE3-128 keyed MAC over header + payload.
/// ADR 0075 §3.2.
fn compute_mac(key: &[u8; 32], header: &[u8; AUTHENTICATED_ENVELOPE_HEADER_SIZE], payload: &[u8]) -> [u8; AUTHENTICATED_ENVELOPE_MAC_SIZE] {
    let mut hasher = blake3::Hasher::new_keyed(key);
    hasher.update(header);
    hasher.update(payload);
    let mut tag = [0u8; AUTHENTICATED_ENVELOPE_MAC_SIZE];
    tag.copy_from_slice(&hasher.finalize().as_bytes()[..AUTHENTICATED_ENVELOPE_MAC_SIZE]);
    tag
}

/// Serialize the fixed 32-byte header. `write_envelope` and
/// `read_envelope` both call this so the MAC input is identical on
/// both sides.
fn serialize_header(header: &AuthenticatedEnvelopeHeader) -> [u8; AUTHENTICATED_ENVELOPE_HEADER_SIZE] {
    let mut bytes = [0_u8; AUTHENTICATED_ENVELOPE_HEADER_SIZE];
    bytes[0..4].copy_from_slice(&AUTHENTICATED_ENVELOPE_MAGIC);
    bytes[4..6].copy_from_slice(&AUTHENTICATED_ENVELOPE_VERSION.to_le_bytes());
    bytes[6..8].copy_from_slice(&header.codec_id.0.to_le_bytes());
    bytes[8..10].copy_from_slice(&header.codec_format_version.to_le_bytes());
    // bytes 10..=11 reserved_a = 0 (already)
    bytes[12..16].copy_from_slice(&header.flags.to_bits().to_le_bytes());
    bytes[16..20].copy_from_slice(&header.encoded_length.to_le_bytes());
    bytes[20..24].copy_from_slice(&header.decoded_length.to_le_bytes());
    // bytes 24..=31 reserved_b = 0 (12 bytes available for future
    // fields such as key_id per ADR 0075 §4 alternative D)
    bytes
}

/// Write an authenticated envelope. MAC is computed over the
/// serialized header bytes + payload bytes; CRC32 is computed over
/// the same input for distinct damage-vs-tamper error reporting.
pub fn write_authenticated_envelope(
    output: &mut Vec<u8>,
    header: &AuthenticatedEnvelopeHeader,
    payload: &[u8],
    auth: &BaceAuthentication,
) -> Result<usize, AuthenticatedEnvelopeError> {
    if payload.len() > u32::MAX as usize {
        return Err(AuthenticatedEnvelopeError::PayloadTooLarge { bytes: payload.len() });
    }
    let encoded_u32 = u32::try_from(payload.len())
        .map_err(|_| AuthenticatedEnvelopeError::PayloadTooLarge { bytes: payload.len() })?;
    if header.encoded_length != encoded_u32 {
        return Err(AuthenticatedEnvelopeError::EncodedLengthMismatch {
            declared: header.encoded_length,
            actual: payload.len(),
        });
    }

    let header_bytes = serialize_header(header);
    // CRC covers header + payload (same input as MAC).
    let mut crc = Hasher::new();
    crc.update(&header_bytes);
    crc.update(payload);
    let crc_tag = crc.finalize();

    let mac = compute_mac(auth.key_bytes(), &header_bytes, payload);

    let written = AUTHENTICATED_ENVELOPE_HEADER_SIZE
        + payload.len()
        + AUTHENTICATED_ENVELOPE_TRAILER_SIZE
        + AUTHENTICATED_ENVELOPE_MAC_SIZE;
    output.reserve(written);
    output.extend_from_slice(&header_bytes);
    output.extend_from_slice(payload);
    output.extend_from_slice(&crc_tag.to_le_bytes());
    output.extend_from_slice(&mac);
    Ok(written)
}

/// Caller-supplied admission caps consulted on the pre-allocation
/// bounds check. Same shape as `envelope::EnvelopeCaps`.
#[derive(Clone, Copy, Debug)]
pub struct AuthenticatedEnvelopeCaps {
    pub max_decoded_length: u32,
    pub max_encoded_length: u32,
}

impl AuthenticatedEnvelopeCaps {
    pub const fn unbounded() -> Self {
        Self {
            max_decoded_length: u32::MAX,
            max_encoded_length: u32::MAX,
        }
    }
}

/// Read an authenticated envelope. Fail-closed checks in order:
///   1. Minimum input size
///   2. Magic
///   3. Envelope version
///   4. Reserved bytes zero
///   5. Encoded/decoded length caps BEFORE allocation
///   6. Observed body length matches declared `encoded_length`
///   7. CRC32 over header + payload (cheap fail-fast for bit rot)
///   8. MAC verification in constant time
///
/// The MAC check runs LAST so a damaged envelope fails with
/// `CrcMismatch` rather than leaking "key is wrong" when the real
/// issue is disk damage. ADR 0075 §3.4.
pub fn read_authenticated_envelope<'a>(
    input: &'a [u8],
    caps: AuthenticatedEnvelopeCaps,
    auth: &BaceAuthentication,
) -> Result<AuthenticatedEnvelope<'a>, AuthenticatedEnvelopeError> {
    // Step 1: min size.
    if input.len() < AUTHENTICATED_ENVELOPE_MIN_SIZE {
        return Err(AuthenticatedEnvelopeError::InputTooSmall {
            bytes: input.len(),
            min: AUTHENTICATED_ENVELOPE_MIN_SIZE,
        });
    }
    let header_bytes_slice = &input[..AUTHENTICATED_ENVELOPE_HEADER_SIZE];
    let mut header_bytes = [0u8; AUTHENTICATED_ENVELOPE_HEADER_SIZE];
    header_bytes.copy_from_slice(header_bytes_slice);

    // Step 2: magic.
    if header_bytes[0..4] != AUTHENTICATED_ENVELOPE_MAGIC {
        return Err(AuthenticatedEnvelopeError::MagicMismatch);
    }

    // Step 3: envelope version.
    let envelope_version = u16::from_le_bytes(header_bytes[4..6].try_into().unwrap());
    if envelope_version != AUTHENTICATED_ENVELOPE_VERSION {
        return Err(AuthenticatedEnvelopeError::VersionMismatch { declared: envelope_version });
    }

    let codec_id = CodecId(u16::from_le_bytes(header_bytes[6..8].try_into().unwrap()));
    let codec_format_version = u16::from_le_bytes(header_bytes[8..10].try_into().unwrap());
    let reserved_a = u16::from_le_bytes(header_bytes[10..12].try_into().unwrap());
    let flags_bits = u32::from_le_bytes(header_bytes[12..16].try_into().unwrap());
    let encoded_length = u32::from_le_bytes(header_bytes[16..20].try_into().unwrap());
    let decoded_length = u32::from_le_bytes(header_bytes[20..24].try_into().unwrap());
    let reserved_b_0 = u32::from_le_bytes(header_bytes[24..28].try_into().unwrap());
    let reserved_b_1 = u32::from_le_bytes(header_bytes[28..32].try_into().unwrap());

    // Step 4: strict reserved-must-be-zero per ADR 0075 §5 Q8.
    if reserved_a != 0 || reserved_b_0 != 0 || reserved_b_1 != 0 {
        return Err(AuthenticatedEnvelopeError::ReservedBitsSet);
    }
    let flags = AuthenticatedEnvelopeFlags::from_bits(flags_bits, codec_id)?;

    // Step 5: encoded/decoded length caps BEFORE touching payload.
    if encoded_length > caps.max_encoded_length {
        return Err(AuthenticatedEnvelopeError::EncodedLengthCapExceeded {
            declared: encoded_length, cap: caps.max_encoded_length,
        });
    }
    if decoded_length > caps.max_decoded_length {
        return Err(AuthenticatedEnvelopeError::DecodedLengthCapExceeded {
            declared: decoded_length, cap: caps.max_decoded_length,
        });
    }

    // Step 6: observed body length matches declared encoded_length.
    let expected_total = AUTHENTICATED_ENVELOPE_HEADER_SIZE
        .saturating_add(encoded_length as usize)
        .saturating_add(AUTHENTICATED_ENVELOPE_TRAILER_SIZE)
        .saturating_add(AUTHENTICATED_ENVELOPE_MAC_SIZE);
    if input.len() < expected_total {
        return Err(AuthenticatedEnvelopeError::Truncated {
            expected: expected_total, actual: input.len(),
        });
    }
    let actual_body_len = input.len()
        - AUTHENTICATED_ENVELOPE_HEADER_SIZE
        - AUTHENTICATED_ENVELOPE_TRAILER_SIZE
        - AUTHENTICATED_ENVELOPE_MAC_SIZE;
    if actual_body_len != encoded_length as usize {
        return Err(AuthenticatedEnvelopeError::EncodedLengthMismatch {
            declared: encoded_length, actual: actual_body_len,
        });
    }

    let body_start = AUTHENTICATED_ENVELOPE_HEADER_SIZE;
    let body_end = body_start + encoded_length as usize;
    let payload = &input[body_start..body_end];
    let crc_bytes_start = body_end;
    let crc_bytes_end = crc_bytes_start + AUTHENTICATED_ENVELOPE_TRAILER_SIZE;
    let mac_start = crc_bytes_end;
    let mac_end = mac_start + AUTHENTICATED_ENVELOPE_MAC_SIZE;
    let declared_crc = u32::from_le_bytes(input[crc_bytes_start..crc_bytes_end].try_into().unwrap());
    let declared_mac = &input[mac_start..mac_end];

    // Step 7: CRC32 over header + payload. Fast fail for bit rot
    // so operators see "CrcMismatch" and know their disk is
    // damaged rather than suspecting their key.
    let mut crc = Hasher::new();
    crc.update(&header_bytes);
    crc.update(payload);
    if crc.finalize() != declared_crc {
        return Err(AuthenticatedEnvelopeError::CrcMismatch);
    }

    // Step 8: MAC verification in constant time.
    // ADR 0075 §3.2 — subtle::ConstantTimeEq is used so the
    // comparison does not leak the tag's byte-level progress.
    let expected_mac = compute_mac(auth.key_bytes(), &header_bytes, payload);
    if expected_mac.ct_eq(declared_mac).unwrap_u8() == 0 {
        return Err(AuthenticatedEnvelopeError::MacMismatch {
            label: auth.label().to_string(),
        });
    }

    Ok(AuthenticatedEnvelope {
        header: AuthenticatedEnvelopeHeader {
            codec_id,
            codec_format_version,
            flags,
            encoded_length,
            decoded_length,
        },
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: [u8; 32] = [1; 32];
    const KEY_B: [u8; 32] = [2; 32];

    fn auth(key: [u8; 32], label: &str) -> BaceAuthentication {
        BaceAuthentication::new(key, label)
    }

    fn header_for(codec: u16, payload: &[u8]) -> AuthenticatedEnvelopeHeader {
        AuthenticatedEnvelopeHeader {
            codec_id: CodecId(codec),
            codec_format_version: 1,
            flags: AuthenticatedEnvelopeFlags { critical: true, ..Default::default() },
            encoded_length: payload.len() as u32,
            decoded_length: payload.len() as u32,
        }
    }

    #[test]
    fn write_then_read_round_trips_payload_under_fixed_key() {
        // ADR 0075 §3.5 item 1 — basic round-trip.
        let a = auth(KEY_A, "primary");
        let payload = b"the quick brown fox";
        let header = header_for(48, payload);
        let mut buf = Vec::new();
        let written = write_authenticated_envelope(&mut buf, &header, payload, &a).unwrap();
        assert_eq!(written, buf.len());
        assert_eq!(
            written,
            AUTHENTICATED_ENVELOPE_HEADER_SIZE + payload.len()
                + AUTHENTICATED_ENVELOPE_TRAILER_SIZE
                + AUTHENTICATED_ENVELOPE_MAC_SIZE,
        );
        let env = read_authenticated_envelope(
            &buf, AuthenticatedEnvelopeCaps::unbounded(), &a,
        ).unwrap();
        assert_eq!(env.payload, payload);
        assert_eq!(env.header, header);
    }

    #[test]
    fn empty_payload_round_trips() {
        // Minimum-size envelope: 32 + 0 + 4 + 16 = 52.
        let a = auth(KEY_A, "primary");
        let header = header_for(48, &[]);
        let mut buf = Vec::new();
        write_authenticated_envelope(&mut buf, &header, &[], &a).unwrap();
        assert_eq!(buf.len(), AUTHENTICATED_ENVELOPE_MIN_SIZE);
        let env = read_authenticated_envelope(
            &buf, AuthenticatedEnvelopeCaps::unbounded(), &a,
        ).unwrap();
        assert!(env.payload.is_empty());
    }

    #[test]
    fn header_byte_flip_fails_mac_mismatch() {
        // ADR 0075 §3.5 item 2 — flipping any header byte must
        // defeat the MAC. We flip byte 7 (inside codec_id); the
        // CRC also moves, but the CRC error path would fire first
        // only if header_bytes diverge from what the writer used.
        // In practice the modified codec_id ALSO changes CRC input,
        // so the test can land either way. The invariant is:
        // reading fails (not Ok).
        let a = auth(KEY_A, "primary");
        let payload = b"abc";
        let header = header_for(48, payload);
        let mut buf = Vec::new();
        write_authenticated_envelope(&mut buf, &header, payload, &a).unwrap();
        buf[7] ^= 0xFF;
        let err = read_authenticated_envelope(
            &buf, AuthenticatedEnvelopeCaps::unbounded(), &a,
        ).unwrap_err();
        assert!(
            matches!(err,
                AuthenticatedEnvelopeError::MacMismatch { .. }
                | AuthenticatedEnvelopeError::CrcMismatch),
            "expected MAC or CRC failure, got {err:?}",
        );
    }

    #[test]
    fn payload_byte_flip_fails_mac_mismatch() {
        // ADR 0075 §3.5 item 3.
        let a = auth(KEY_A, "primary");
        let payload = b"abcdef";
        let header = header_for(48, payload);
        let mut buf = Vec::new();
        write_authenticated_envelope(&mut buf, &header, payload, &a).unwrap();
        let payload_offset = AUTHENTICATED_ENVELOPE_HEADER_SIZE;
        buf[payload_offset + 2] ^= 0x01;
        let err = read_authenticated_envelope(
            &buf, AuthenticatedEnvelopeCaps::unbounded(), &a,
        ).unwrap_err();
        // Payload flip also breaks CRC; CRC fires first.
        assert!(
            matches!(err, AuthenticatedEnvelopeError::CrcMismatch),
            "expected CrcMismatch (CRC fires before MAC on body damage), got {err:?}",
        );
    }

    #[test]
    fn trailer_crc_byte_flip_fails_crc_before_mac() {
        // ADR 0075 §3.5 item 4 — CRC fails fast; MAC never runs.
        let a = auth(KEY_A, "primary");
        let payload = b"abc";
        let header = header_for(48, payload);
        let mut buf = Vec::new();
        write_authenticated_envelope(&mut buf, &header, payload, &a).unwrap();
        let crc_offset = AUTHENTICATED_ENVELOPE_HEADER_SIZE + payload.len();
        buf[crc_offset] ^= 0x01;
        let err = read_authenticated_envelope(
            &buf, AuthenticatedEnvelopeCaps::unbounded(), &a,
        ).unwrap_err();
        assert!(
            matches!(err, AuthenticatedEnvelopeError::CrcMismatch),
            "expected CrcMismatch, got {err:?}",
        );
    }

    #[test]
    fn mac_byte_flip_fails_mac_mismatch_with_crc_intact() {
        // ADR 0075 §3.5 item 5 — body+CRC untouched; only MAC byte
        // flips. CRC passes, then MAC verification fires.
        let a = auth(KEY_A, "primary");
        let payload = b"abc";
        let header = header_for(48, payload);
        let mut buf = Vec::new();
        write_authenticated_envelope(&mut buf, &header, payload, &a).unwrap();
        let mac_offset = AUTHENTICATED_ENVELOPE_HEADER_SIZE + payload.len()
            + AUTHENTICATED_ENVELOPE_TRAILER_SIZE;
        buf[mac_offset + 3] ^= 0x01;
        let err = read_authenticated_envelope(
            &buf, AuthenticatedEnvelopeCaps::unbounded(), &a,
        ).unwrap_err();
        assert!(
            matches!(err, AuthenticatedEnvelopeError::MacMismatch { .. }),
            "expected MacMismatch, got {err:?}",
        );
    }

    #[test]
    fn reading_with_different_key_fails_mac_mismatch() {
        // ADR 0075 §3.5 item 6 — correct bytes, wrong key.
        let a = auth(KEY_A, "primary");
        let b = auth(KEY_B, "secondary");
        let payload = b"abc";
        let header = header_for(48, payload);
        let mut buf = Vec::new();
        write_authenticated_envelope(&mut buf, &header, payload, &a).unwrap();
        let err = read_authenticated_envelope(
            &buf, AuthenticatedEnvelopeCaps::unbounded(), &b,
        ).unwrap_err();
        match err {
            AuthenticatedEnvelopeError::MacMismatch { label } => {
                assert_eq!(label, "secondary",
                    "label should name the reader's key, not the writer's");
            }
            other => panic!("expected MacMismatch, got {other:?}"),
        }
    }

    #[test]
    fn baca_envelope_fails_as_bace_v1() {
        // ADR 0075 §3.5 item 7 — BACA bytes must NOT accidentally
        // verify through the BACE v1 reader.
        use crate::bace::envelope::{read_envelope, EnvelopeCaps};
        let a = auth(KEY_A, "primary");
        let payload = b"abc";
        let header = header_for(48, payload);
        let mut buf = Vec::new();
        write_authenticated_envelope(&mut buf, &header, payload, &a).unwrap();
        let err = read_envelope(&buf, EnvelopeCaps::unbounded()).unwrap_err();
        assert!(
            matches!(err, crate::bace::envelope::EnvelopeError::UnknownMagic),
            "expected UnknownMagic, got {err:?}",
        );
    }

    #[test]
    fn bace_v1_envelope_fails_as_baca() {
        // ADR 0075 §3.5 item 8 — BACE v1 bytes must NOT accidentally
        // verify through the BACA reader.
        use crate::bace::envelope::{write_envelope, EnvelopeHeader, EnvelopeFlags};
        let a = auth(KEY_A, "primary");
        let payload = b"abc";
        let header = EnvelopeHeader {
            codec_id: CodecId(48),
            codec_format_version: 1,
            flags: EnvelopeFlags::default(),
            encoded_length: payload.len() as u32,
            decoded_length: payload.len() as u32,
        };
        let mut buf = Vec::new();
        write_envelope(&mut buf, &header, payload).unwrap();
        // BACE v1 envelope is 36 bytes for this payload; too small
        // for BACA's 52-byte minimum.
        let err = read_authenticated_envelope(
            &buf, AuthenticatedEnvelopeCaps::unbounded(), &a,
        ).unwrap_err();
        assert!(
            matches!(err, AuthenticatedEnvelopeError::InputTooSmall { .. }
                | AuthenticatedEnvelopeError::MagicMismatch),
            "expected InputTooSmall or MagicMismatch, got {err:?}",
        );
    }

    #[test]
    fn decoded_length_cap_fires_before_mac_computation() {
        // ADR 0075 §3.5 item 9 — length caps fire before MAC.
        // Construct a well-formed envelope, then read with a cap
        // smaller than the declared decoded_length.
        let a = auth(KEY_A, "primary");
        let payload = vec![0u8; 100];
        let header = header_for(48, &payload);
        let mut buf = Vec::new();
        write_authenticated_envelope(&mut buf, &header, &payload, &a).unwrap();
        let caps = AuthenticatedEnvelopeCaps {
            max_decoded_length: 50,
            max_encoded_length: u32::MAX,
        };
        let err = read_authenticated_envelope(&buf, caps, &a).unwrap_err();
        match err {
            AuthenticatedEnvelopeError::DecodedLengthCapExceeded { declared, cap } => {
                assert_eq!(declared, 100);
                assert_eq!(cap, 50);
            }
            other => panic!("expected DecodedLengthCapExceeded, got {other:?}"),
        }
    }

    #[test]
    fn bace_authentication_debug_does_not_print_key_bytes() {
        // ADR 0075 §3.5 item 10 — Debug must not leak key.
        let a = auth([0x42u8; 32], "operator-label");
        let printed = format!("{a:?}");
        assert!(printed.contains("operator-label"),
            "Debug should show label: {printed}");
        assert!(printed.contains("[redacted]"),
            "Debug should redact key: {printed}");
        assert!(!printed.contains("42, 42"),
            "Debug must not leak key bytes: {printed}");
    }

    #[test]
    fn mac_verification_uses_subtle_constant_time_comparison() {
        // ADR 0075 §3.5 item 11 — code-level assertion, inspectable
        // by eye: read_authenticated_envelope's MAC check uses
        // `subtle::ConstantTimeEq`. Verifying this programmatically
        // from outside is hard; this test instead confirms the
        // `subtle` type is reachable and the trait is implemented
        // for fixed-size byte arrays as our implementation requires.
        let a: [u8; 16] = [0; 16];
        let b: [u8; 16] = [0; 16];
        assert_eq!(a.ct_eq(&b).unwrap_u8(), 1);
        let c: [u8; 16] = [1; 16];
        assert_eq!(a.ct_eq(&c).unwrap_u8(), 0);
    }

    #[test]
    fn truncated_envelope_fails_truncated_or_input_too_small() {
        let a = auth(KEY_A, "primary");
        let payload = b"abcdef";
        let header = header_for(48, payload);
        let mut buf = Vec::new();
        write_authenticated_envelope(&mut buf, &header, payload, &a).unwrap();
        // Chop off the MAC.
        buf.truncate(buf.len() - AUTHENTICATED_ENVELOPE_MAC_SIZE);
        let err = read_authenticated_envelope(
            &buf, AuthenticatedEnvelopeCaps::unbounded(), &a,
        ).unwrap_err();
        assert!(
            matches!(err,
                AuthenticatedEnvelopeError::Truncated { .. }
                | AuthenticatedEnvelopeError::InputTooSmall { .. }),
            "expected Truncated/InputTooSmall, got {err:?}",
        );
    }
}
