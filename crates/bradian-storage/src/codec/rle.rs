//! Reserved RLE codec. Not produced by v1 writers. Reader returns `UnsupportedFeature`
//! unless the segment marks this codec as optional.
//!
//! Format (when implemented):
//!   u32 n_rows
//!   (u32 run_length, i64 value)*  -- fixed to i64 initially; parametrised later.

use bradian_core::{BradianError, BradianResult};

pub fn decode(_bytes: &[u8]) -> BradianResult<()> {
    Err(BradianError::unsupported("RLE codec not implemented in v1"))
}
