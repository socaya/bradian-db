//! Reserved Delta / Delta-of-Delta codec. Not produced by v1 writers.

use dap_core::{DapError, DapResult};

pub fn decode(_bytes: &[u8]) -> DapResult<()> {
    Err(DapError::unsupported("Delta codec not implemented in v1"))
}
