//! Plain little-endian encoding for fixed-width primitive types.
//!
//! The encoded buffer is simply the row-major concatenation of little-endian values.
//! Nulls occupy a slot (whose value is undefined by contract) — the validity bitmap is encoded
//! separately in the page header. This keeps the encoder branchless and the decoder a straight
//! memcpy.

use dap_core::{DapError, DapResult};

use crate::column::ColumnBuffer;

macro_rules! encode_primitive {
    ($data:expr, $t:ty) => {{
        let mut out = Vec::with_capacity($data.len() * std::mem::size_of::<$t>());
        for v in $data {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }};
}

macro_rules! decode_primitive {
    ($bytes:expr, $t:ty) => {{
        const W: usize = std::mem::size_of::<$t>();
        if $bytes.len() % W != 0 {
            return Err(DapError::corruption(format!(
                "plain {} buffer has {} bytes, not a multiple of {}",
                stringify!($t),
                $bytes.len(),
                W
            )));
        }
        let n = $bytes.len() / W;
        let mut out = Vec::with_capacity(n);
        for chunk in $bytes.chunks_exact(W) {
            let mut buf = [0u8; W];
            buf.copy_from_slice(chunk);
            out.push(<$t>::from_le_bytes(buf));
        }
        out
    }};
}

pub fn encode(buf: &ColumnBuffer) -> DapResult<Vec<u8>> {
    match buf {
        ColumnBuffer::I32 { data, .. } => Ok(encode_primitive!(data, i32)),
        ColumnBuffer::I64 { data, .. } => Ok(encode_primitive!(data, i64)),
        ColumnBuffer::F64 { data, .. } => Ok(encode_primitive!(data, f64)),
        ColumnBuffer::Timestamp { data, .. } => Ok(encode_primitive!(data, i64)),
        ColumnBuffer::Decimal128 { data, .. } => {
            let mut out = Vec::with_capacity(data.len() * 16);
            for v in data {
                out.extend_from_slice(&v.to_le_bytes());
            }
            Ok(out)
        }
        ColumnBuffer::Bool { data, .. } => Ok(data.clone()),
        other => Err(DapError::unsupported(format!(
            "plain codec cannot encode {:?}",
            other.physical_type()
        ))),
    }
}

pub fn decode_i32(bytes: &[u8]) -> DapResult<Vec<i32>> {
    Ok(decode_primitive!(bytes, i32))
}
pub fn decode_i64(bytes: &[u8]) -> DapResult<Vec<i64>> {
    Ok(decode_primitive!(bytes, i64))
}
pub fn decode_f64(bytes: &[u8]) -> DapResult<Vec<f64>> {
    Ok(decode_primitive!(bytes, f64))
}
pub fn decode_decimal128(bytes: &[u8]) -> DapResult<Vec<i128>> {
    if !bytes.len().is_multiple_of(16) {
        return Err(DapError::corruption(format!(
            "plain decimal128 buffer has {} bytes, not multiple of 16",
            bytes.len()
        )));
    }
    let n = bytes.len() / 16;
    let mut out = Vec::with_capacity(n);
    for chunk in bytes.as_chunks::<16>().0.iter() {
        let mut buf = [0u8; 16];
        buf.copy_from_slice(chunk);
        out.push(i128::from_le_bytes(buf));
    }
    Ok(out)
}
pub fn decode_bool(bytes: &[u8]) -> DapResult<Vec<u8>> {
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitmap::Bitmap;

    #[test]
    fn i64_roundtrip() {
        let buf = ColumnBuffer::I64 {
            data: vec![1, -2, 1 << 40, -(1 << 60)],
            validity: Bitmap::all_set(4),
        };
        let enc = encode(&buf).unwrap();
        assert_eq!(decode_i64(&enc).unwrap(), vec![1, -2, 1 << 40, -(1 << 60)]);
    }

    #[test]
    fn f64_roundtrip() {
        let buf = ColumnBuffer::F64 {
            data: vec![1.5, -3.25, 0.0, f64::INFINITY],
            validity: Bitmap::all_set(4),
        };
        let enc = encode(&buf).unwrap();
        let back = decode_f64(&enc).unwrap();
        assert_eq!(back, vec![1.5, -3.25, 0.0, f64::INFINITY]);
    }

    #[test]
    fn decimal128_roundtrip() {
        let buf = ColumnBuffer::Decimal128 {
            data: vec![0i128, i128::MAX, i128::MIN, 42_000],
            validity: Bitmap::all_set(4),
            scale: 2,
        };
        let enc = encode(&buf).unwrap();
        assert_eq!(decode_decimal128(&enc).unwrap(), vec![0, i128::MAX, i128::MIN, 42_000]);
    }
}
