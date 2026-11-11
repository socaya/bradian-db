//! `.derc` v1 header, footer and page metadata.

use dap_core::{Fingerprint, LogicalType, PhysicalType};
use serde::{Deserialize, Serialize};

use crate::zonemap::ZoneMap;

pub const SEGMENT_MAGIC: [u8; 8] = *b"DERC\x01SEG";
pub const SEGMENT_VERSION_MAJOR: u16 = 1;
pub const SEGMENT_VERSION_MINOR: u16 = 0;

/// Required feature flags (bit positions).
#[derive(Clone, Copy, Debug)]
pub struct FeatureFlags(pub u64);

impl FeatureFlags {
    pub const COMPRESSED_PAGES: u64 = 1 << 0;
    pub const DICT_ENCODING: u64 = 1 << 1;
    pub const BITPACK_ENCODING: u64 = 1 << 2;
    pub const DELTA_ENCODING: u64 = 1 << 3;
    pub const PAGE_CRC32: u64 = 1 << 4;

    pub const ALL_SUPPORTED_V1: u64 =
        Self::COMPRESSED_PAGES | Self::DICT_ENCODING | Self::PAGE_CRC32;

    pub fn is_subset_supported(required: u64) -> bool {
        required & !Self::ALL_SUPPORTED_V1 == 0
    }
}

/// Codec IDs as persisted in page meta. Must match `codec::PageCodec`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum CodecId {
    Plain = 0,
    PlainUtf8 = 1,
    Dictionary = 2,
    RunLength = 3,
    Delta = 4,
    BitPacked = 5,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PageMeta {
    /// Codec used to encode the uncompressed inner payload.
    pub codec: CodecId,
    /// True iff `payload_bytes` is a zstd frame wrapping the codec output.
    pub compressed: bool,
    /// Absolute byte offset of the compressed payload in the file.
    pub payload_offset: u64,
    /// Length of the payload on disk (may be the compressed length).
    pub payload_len: u32,
    /// Length of the uncompressed codec output. Equal to `payload_len` when `compressed=false`.
    pub uncompressed_len: u32,
    /// Absolute byte offset of the validity bitmap (if any).
    pub validity_offset: u64,
    /// Length on disk of the validity bitmap (0 iff the column is non-nullable).
    pub validity_len: u32,
    /// Row count in this page (not the byte count).
    pub row_count: u32,
    /// Zone map (min/max/null_count/row_count).
    pub zone_map: ZoneMap,
    /// CRC32 of (compressed-payload || validity-bitmap).
    pub crc32: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ColumnMeta {
    pub name: String,
    pub logical_type: LogicalType,
    pub physical_type: PhysicalType,
    pub nullable: bool,
    pub pages: Vec<PageMeta>,
    /// Merged zone map across pages.
    pub zone_map: ZoneMap,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SegmentFooter {
    pub dataset_id: String,
    pub partition_id: String,
    pub segment_id: String,
    pub schema_fingerprint: Fingerprint,
    /// Pointer to the catalog metadata revision the segment was written against.
    pub metadata_revision: Option<Fingerprint>,
    pub row_count: u64,
    pub columns: Vec<ColumnMeta>,
    /// Max ingest watermark captured when the segment was sealed (ms since epoch).
    pub watermark_ms: i64,
    /// Source-system LSN or opaque identity of the data revision represented.
    pub data_revision: u64,
}
