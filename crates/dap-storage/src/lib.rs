//! DAP storage: native columnar segments (`.derc`), WAL, manifests, delta, compaction.
//!
//! Format spec: `docs/adr/0002-derc-segment-format.md`.
//!
//! Hot paths avoid `unsafe`. The one place we work with raw bytes (page codecs) stays within
//! documented bounds-checked slices.
#![deny(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod bitmap;
pub mod checksum;
pub mod codec;
pub mod column;
pub mod partition;
pub mod segment;
pub mod zonemap;

pub use column::{ColumnBuffer, ColumnValue};
pub use partition::{
    Delta, DeltaEntry, Manifest, ManifestSegment, Partition, PartitionOptions, PrimaryKey, Row,
    Snapshot, WalReader, WalRecord, WalWriter,
};
pub use segment::{
    CodecId, ColumnMeta, FeatureFlags, PageMeta, SegmentFooter, SegmentReader, SegmentWriter,
    SegmentWriterOptions, SEGMENT_MAGIC, SEGMENT_VERSION_MAJOR, SEGMENT_VERSION_MINOR,
};
pub use zonemap::ZoneMap;
