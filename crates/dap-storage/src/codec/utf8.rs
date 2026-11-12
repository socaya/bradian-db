//! Plain UTF-8 encoding: `u32` offsets followed by the concatenated bytes.
//!
//! Layout:
//!   u32 n_rows
//!   u32[n_rows + 1] offsets (cumulative byte offsets starting at 0)
//!   u8[] data

use dap_core::{DapError, DapResult};

use crate::column::ColumnBuffer;

pub fn encode(buf: &ColumnBuffer) -> DapResult<Vec<u8>> {
    match buf {
        ColumnBuffer::Utf8 { offsets, data, .. } => {
            let n = (offsets.len() - 1) as u32;
            let mut out = Vec::with_capacity(4 + offsets.len() * 4 + data.len());
            out.extend_from_slice(&n.to_le_bytes());
            for o in offsets {
                out.extend_from_slice(&o.to_le_bytes());
            }
            out.extend_from_slice(data);
            Ok(out)
        }
        other => Err(DapError::unsupported(format!(
            "utf8 codec cannot encode {:?}",
            other.physical_type()
        ))),
    }
}

pub fn decode(bytes: &[u8]) -> DapResult<(Vec<u32>, Vec<u8>)> {
    if bytes.len() < 4 {
        return Err(DapError::corruption("utf8 page too short"));
    }
    let n = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    let off_end = 4 + (n + 1) * 4;
    if bytes.len() < off_end {
        return Err(DapError::corruption("utf8 page truncated offsets"));
    }
    let mut offsets = Vec::with_capacity(n + 1);
    for chunk in bytes[4..off_end].as_chunks::<4>().0.iter() {
        offsets.push(u32::from_le_bytes(*chunk));
    }
    let data = bytes[off_end..].to_vec();
    let expected = *offsets.last().unwrap_or(&0) as usize;
    if data.len() != expected {
        return Err(DapError::corruption(format!(
            "utf8 data length {} does not match last offset {}",
            data.len(),
            expected
        )));
    }
    Ok((offsets, data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitmap::Bitmap;

    #[test]
    fn empty_and_values() {
        let buf = ColumnBuffer::Utf8 {
            offsets: vec![0, 3, 3, 11],
            data: b"derwoRLDNOW".to_vec(),
            validity: Bitmap::all_set(3),
        };
        let enc = encode(&buf).unwrap();
        let (offsets, data) = decode(&enc).unwrap();
        assert_eq!(offsets, vec![0, 3, 3, 11]);
        assert_eq!(data, b"derwoRLDNOW");
    }
}
