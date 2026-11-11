//! `.derc` segment format v1 — writer + reader.
//!
//! Byte layout and feature flags are specified in `docs/adr/0002-derc-segment-format.md`.

mod format;
mod reader;
mod writer;

pub use format::{
    CodecId, ColumnMeta, FeatureFlags, PageMeta, SegmentFooter, SEGMENT_MAGIC,
    SEGMENT_VERSION_MAJOR, SEGMENT_VERSION_MINOR,
};
pub use reader::SegmentReader;
pub use writer::{SegmentWriter, SegmentWriterOptions};
