//! Reserved Delta / Delta-of-Delta codec. Not produced by v1 writers.

use bradian_core::{BradianError, BradianResult};

pub fn decode(_bytes: &[u8]) -> BradianResult<()> {
    Err(BradianError::unsupported("Delta codec not implemented in v1"))
}
