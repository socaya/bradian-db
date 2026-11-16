//! ADR 0137 — wire-frame envelope primitive.
//!
//! BRD-WIRE-001 "portable driver wire protocol" is OPEN.
//! ADR 0129 shipped the `Driver` trait; this module ships
//! the on-wire PACKET FORMAT a future network driver will
//! use between client and server.
//!
//! Frame layout (big-endian; same convention as the rest
//! of Bradian's on-disk formats):
//!
//! ```text
//! +--------+--------+----------+----------+------------+---------+
//! | MAGIC  | ver    | kind     | reserved | payload_len|  CRC32  |
//! | BRFR\1 | u16    | u8       | u8       | u32        | u32     |
//! +--------+--------+----------+----------+------------+---------+
//! | payload_len bytes of opaque payload                           |
//! +--------------------------------------------------------------+
//! | trailing CRC32 covers header + payload                        |
//! ```
//!
//! Honest scope:
//! - Fixed 16-byte header.
//! - `encode(kind, payload)` returns the full frame bytes.
//! - `decode(bytes)` returns `(kind, payload)` or an error.
//! - Length bound: `MAX_PAYLOAD = 16 * 1024 * 1024` (16 MiB)
//!   to prevent memory exhaustion from a bogus header.
//! - CRC32 (ieee) covers the complete header (with len
//!   field set) + payload; placed in the trailing 4 bytes.
//!
//! Out of scope (follow-ups):
//! - TLS/mTLS framing — this is the in-stream envelope
//!   only; transport security is per-connection.
//! - Compression — payload bytes are opaque; a caller can
//!   pre-compress.
//! - Fragmentation — frames carry up to MAX_PAYLOAD bytes
//!   in one packet. Larger logical messages must split.
//! - Authentication — BACE/BACA envelopes (ADR 0075+)
//!   apply at a higher layer and go in the payload.
//! - Version negotiation — `ver` is monotone; the server
//!   rejects unknown versions. Downgrade logic is a
//!   follow-up.

use thiserror::Error;

pub const FRAME_MAGIC: [u8; 4] = *b"BRFR";
pub const FRAME_VERSION: u16 = 1;
pub const FRAME_HEADER_SIZE: usize = 16;
pub const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

/// Frame payload categories. Narrow by design; follow-up
/// ADRs add negotiation / auth-challenge / streaming-row
/// kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    /// Opaque request from client to server.
    Request = 1,
    /// Opaque response from server to client.
    Response = 2,
    /// Lightweight keepalive; payload typically empty.
    KeepAlive = 3,
    /// Error delivery; payload is a UTF-8 message.
    Error = 4,
}

impl FrameKind {
    fn from_u8(b: u8) -> Option<Self> {
        match b {
            1 => Some(FrameKind::Request),
            2 => Some(FrameKind::Response),
            3 => Some(FrameKind::KeepAlive),
            4 => Some(FrameKind::Error),
            _ => None,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("frame too short: {got} bytes, need at least {}", FRAME_HEADER_SIZE)]
    TooShort { got: usize },
    #[error("bad magic: expected {:?}, got {got:?}", FRAME_MAGIC)]
    BadMagic { got: [u8; 4] },
    #[error("unsupported wire version {got} (this build speaks {})", FRAME_VERSION)]
    UnsupportedVersion { got: u16 },
    #[error("unknown frame kind {got}")]
    UnknownKind { got: u8 },
    #[error("payload length {got} exceeds MAX_PAYLOAD {max}")]
    PayloadTooLarge { got: usize, max: usize },
    #[error("frame length mismatch: header says {header_len}, actual {actual_len}")]
    LengthMismatch { header_len: usize, actual_len: usize },
    #[error("CRC32 mismatch: expected {expected:#010x}, got {actual:#010x}")]
    ChecksumMismatch { expected: u32, actual: u32 },
}

pub type FrameResult<T> = std::result::Result<T, FrameError>;

/// ADR 0137 — encode `(kind, payload)` as a complete
/// wire frame. Returns the owned byte vec; caller writes
/// it to the transport as one unit.
pub fn encode(kind: FrameKind, payload: &[u8]) -> FrameResult<Vec<u8>> {
    if payload.len() > MAX_PAYLOAD {
        return Err(FrameError::PayloadTooLarge {
            got: payload.len(),
            max: MAX_PAYLOAD,
        });
    }
    let total = FRAME_HEADER_SIZE + payload.len() + 4; // + trailing CRC
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&FRAME_MAGIC);
    out.extend_from_slice(&FRAME_VERSION.to_be_bytes());
    out.push(kind as u8);
    out.push(0); // reserved
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    // Zero placeholder for the inner CRC field so we can
    // checksum the stable form.
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(payload);
    let checksum = crc32fast::hash(&out);
    out.extend_from_slice(&checksum.to_be_bytes());
    Ok(out)
}

/// ADR 0137 — decode a complete wire frame. Returns
/// `(kind, payload_bytes)`. The caller must have already
/// slurped the full frame length — this primitive does not
/// read from a stream.
pub fn decode(bytes: &[u8]) -> FrameResult<(FrameKind, Vec<u8>)> {
    if bytes.len() < FRAME_HEADER_SIZE + 4 {
        return Err(FrameError::TooShort { got: bytes.len() });
    }
    let magic: [u8; 4] = bytes[0..4].try_into().unwrap();
    if magic != FRAME_MAGIC {
        return Err(FrameError::BadMagic { got: magic });
    }
    let ver = u16::from_be_bytes([bytes[4], bytes[5]]);
    if ver != FRAME_VERSION {
        return Err(FrameError::UnsupportedVersion { got: ver });
    }
    let kind = FrameKind::from_u8(bytes[6])
        .ok_or(FrameError::UnknownKind { got: bytes[6] })?;
    // byte 7 is reserved (ignored)
    let payload_len = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    if payload_len > MAX_PAYLOAD {
        return Err(FrameError::PayloadTooLarge { got: payload_len, max: MAX_PAYLOAD });
    }
    let total = FRAME_HEADER_SIZE + payload_len + 4;
    if bytes.len() != total {
        return Err(FrameError::LengthMismatch {
            header_len: total,
            actual_len: bytes.len(),
        });
    }
    let actual_crc = u32::from_be_bytes([
        bytes[total - 4], bytes[total - 3], bytes[total - 2], bytes[total - 1],
    ]);
    // Recompute over header (with placeholder zeroed) + payload.
    let mut canonical = bytes[0..total - 4].to_vec();
    // zero the 12..16 placeholder window
    canonical[12..16].copy_from_slice(&[0u8; 4]);
    let expected_crc = crc32fast::hash(&canonical);
    if expected_crc != actual_crc {
        return Err(FrameError::ChecksumMismatch {
            expected: expected_crc,
            actual: actual_crc,
        });
    }
    let payload = bytes[FRAME_HEADER_SIZE..FRAME_HEADER_SIZE + payload_len].to_vec();
    Ok((kind, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_request() {
        let payload = b"SELECT 1";
        let frame = encode(FrameKind::Request, payload).unwrap();
        let (kind, body) = decode(&frame).unwrap();
        assert_eq!(kind, FrameKind::Request);
        assert_eq!(body, payload);
    }

    #[test]
    fn round_trip_all_kinds() {
        for &k in &[FrameKind::Request, FrameKind::Response,
                    FrameKind::KeepAlive, FrameKind::Error] {
            let frame = encode(k, &[0xde, 0xad, 0xbe, 0xef]).unwrap();
            let (back_kind, _) = decode(&frame).unwrap();
            assert_eq!(k, back_kind);
        }
    }

    #[test]
    fn empty_payload_round_trips() {
        let frame = encode(FrameKind::KeepAlive, &[]).unwrap();
        let (kind, body) = decode(&frame).unwrap();
        assert_eq!(kind, FrameKind::KeepAlive);
        assert!(body.is_empty());
    }

    #[test]
    fn too_short_rejected() {
        let err = decode(&[0u8; 5]).unwrap_err();
        assert!(matches!(err, FrameError::TooShort { .. }));
    }

    #[test]
    fn bad_magic_rejected() {
        let mut frame = encode(FrameKind::Request, b"x").unwrap();
        frame[0] = b'X';
        let err = decode(&frame).unwrap_err();
        assert!(matches!(err, FrameError::BadMagic { .. }));
    }

    #[test]
    fn unsupported_version_rejected() {
        let mut frame = encode(FrameKind::Request, b"x").unwrap();
        frame[5] = 99;   // bump version
        let err = decode(&frame).unwrap_err();
        assert!(matches!(err, FrameError::UnsupportedVersion { .. }));
    }

    #[test]
    fn unknown_kind_rejected() {
        let mut frame = encode(FrameKind::Request, b"x").unwrap();
        frame[6] = 42;
        let err = decode(&frame).unwrap_err();
        assert!(matches!(err, FrameError::UnknownKind { got: 42 }));
    }

    #[test]
    fn length_mismatch_rejected() {
        let mut frame = encode(FrameKind::Request, b"abcdefgh").unwrap();
        frame.pop(); // chop a byte
        let err = decode(&frame).unwrap_err();
        assert!(matches!(err, FrameError::LengthMismatch { .. }));
    }

    #[test]
    fn bit_flip_in_payload_detected_by_crc() {
        let mut frame = encode(FrameKind::Request, b"abcdefgh").unwrap();
        frame[FRAME_HEADER_SIZE + 2] ^= 0x01;
        let err = decode(&frame).unwrap_err();
        assert!(matches!(err, FrameError::ChecksumMismatch { .. }));
    }

    #[test]
    fn oversize_payload_rejected_on_encode() {
        let huge = vec![0u8; MAX_PAYLOAD + 1];
        let err = encode(FrameKind::Request, &huge).unwrap_err();
        assert!(matches!(err, FrameError::PayloadTooLarge { .. }));
    }

    #[test]
    fn oversize_payload_rejected_on_decode() {
        // Craft a header that lies about payload size.
        let mut frame = Vec::new();
        frame.extend_from_slice(&FRAME_MAGIC);
        frame.extend_from_slice(&FRAME_VERSION.to_be_bytes());
        frame.push(FrameKind::Request as u8);
        frame.push(0);
        frame.extend_from_slice(&(((MAX_PAYLOAD + 1) as u32).to_be_bytes()));
        frame.extend_from_slice(&[0u8; 4]);
        frame.extend_from_slice(&[0u8; 4]); // bogus crc tail
        let err = decode(&frame).unwrap_err();
        assert!(matches!(err, FrameError::PayloadTooLarge { .. }));
    }

    #[test]
    fn reserved_byte_is_ignored_on_decode() {
        let mut frame = encode(FrameKind::Request, b"x").unwrap();
        frame[7] = 0xff;
        // Change invalidates CRC — recompute to confirm we only
        // reject on CRC, not on reserved byte alone.
        let mut canonical = frame[0..FRAME_HEADER_SIZE + 1].to_vec();
        canonical[12..16].copy_from_slice(&[0u8; 4]);
        let new_crc = crc32fast::hash(&canonical);
        let total = frame.len();
        frame[total - 4..].copy_from_slice(&new_crc.to_be_bytes());
        let (kind, _) = decode(&frame).unwrap();
        assert_eq!(kind, FrameKind::Request);
    }
}
