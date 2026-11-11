//! Page codecs. Each codec encodes a dense typed buffer into bytes; the reader decodes back.
//!
//! The v1 release wires codec IDs 0 (Plain), 1 (PlainUtf8) and 2 (Dictionary). The remaining
//! IDs (RLE, Delta, BitPack) are reserved in the format so segments can be forward-compatibly
//! read once encoders land. See `docs/adr/0002-derc-segment-format.md` §Codec IDs.

use serde::{Deserialize, Serialize};

pub mod bitpack;
pub mod delta;
pub mod dict;
pub mod plain;
pub mod rle;
pub mod utf8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum PageCodec {
    Plain = 0,
    PlainUtf8 = 1,
    Dictionary = 2,
    RunLength = 3,
    Delta = 4,
    BitPacked = 5,
}

impl PageCodec {
    pub fn from_u8(x: u8) -> Option<Self> {
        Some(match x {
            0 => PageCodec::Plain,
            1 => PageCodec::PlainUtf8,
            2 => PageCodec::Dictionary,
            3 => PageCodec::RunLength,
            4 => PageCodec::Delta,
            5 => PageCodec::BitPacked,
            _ => return None,
        })
    }
}

/// zstd wrapper used by all page codecs by default. Level kept modest (3) — the hot path for
/// edge devices is read-time decompression, not maximal compression ratio.
pub mod compress {
    use dap_core::{DapError, DapResult};

    pub const DEFAULT_LEVEL: i32 = 3;

    pub fn compress(bytes: &[u8]) -> DapResult<Vec<u8>> {
        zstd::stream::encode_all(bytes, DEFAULT_LEVEL).map_err(DapError::from)
    }

    pub fn decompress(bytes: &[u8], expected_len: usize) -> DapResult<Vec<u8>> {
        let out = zstd::stream::decode_all(bytes).map_err(DapError::from)?;
        if out.len() != expected_len {
            return Err(DapError::corruption(format!(
                "page decompresses to {} bytes, expected {}",
                out.len(),
                expected_len
            )));
        }
        Ok(out)
    }
}
