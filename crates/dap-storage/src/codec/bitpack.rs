//! Reserved BitPack codec. Not produced by v1 writers.

use dap_core::{DapError, DapResult};

pub fn decode(_bytes: &[u8]) -> DapResult<()> {
    Err(DapError::unsupported("BitPack codec not implemented in v1"))
}
