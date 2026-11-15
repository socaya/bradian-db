//! Reserved BitPack codec. Not produced by v1 writers.

use bradian_core::{BradianError, BradianResult};

pub fn decode(_bytes: &[u8]) -> BradianResult<()> {
    Err(BradianError::unsupported("BitPack codec not implemented in v1"))
}
