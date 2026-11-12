//! Compact validity bitmap. LSB-first per byte; one bit per row.
//!
//! `set=true` means the value at that row is valid (non-null). This matches Arrow conventions.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Bitmap {
    bytes: Vec<u8>,
    len: usize,
    null_count: usize,
}

impl Bitmap {
    pub fn with_capacity(len: usize) -> Self {
        let bytes = vec![0u8; len.div_ceil(8)];
        Self { bytes, len: 0, null_count: 0 }
    }

    /// Build a bitmap where every row is marked non-null.
    pub fn all_set(len: usize) -> Self {
        let mut bytes = vec![0xFFu8; len.div_ceil(8)];
        // Clear trailing bits beyond `len` so popcount is accurate.
        if !len.is_multiple_of(8) {
            let last = bytes.len() - 1;
            let keep_bits = (len % 8) as u32;
            let mask = (1u8 << keep_bits) - 1;
            bytes[last] &= mask;
        }
        Self { bytes, len, null_count: 0 }
    }

    pub fn from_bytes(bytes: Vec<u8>, len: usize) -> Self {
        let need = len.div_ceil(8);
        let bytes = if bytes.len() == need {
            bytes
        } else {
            let mut b = bytes;
            b.resize(need, 0);
            b
        };
        // Count nulls = rows with bit=0 in the first `len` bits.
        let mut set = 0usize;
        for i in 0..len {
            let byte = bytes[i / 8];
            if (byte >> (i % 8)) & 1 == 1 {
                set += 1;
            }
        }
        let null_count = len - set;
        Self { bytes, len, null_count }
    }

    pub fn push(&mut self, valid: bool) {
        let idx = self.len;
        let byte_idx = idx / 8;
        if byte_idx >= self.bytes.len() {
            self.bytes.push(0);
        }
        if valid {
            self.bytes[byte_idx] |= 1 << (idx % 8);
        } else {
            self.null_count += 1;
        }
        self.len += 1;
    }

    pub fn is_valid(&self, idx: usize) -> bool {
        debug_assert!(idx < self.len);
        (self.bytes[idx / 8] >> (idx % 8)) & 1 == 1
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn null_count(&self) -> usize {
        self.null_count
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_read() {
        let mut b = Bitmap::with_capacity(0);
        let pattern = [true, false, true, true, false, false, true, false, true];
        for v in pattern {
            b.push(v);
        }
        assert_eq!(b.len(), pattern.len());
        for (i, v) in pattern.iter().enumerate() {
            assert_eq!(b.is_valid(i), *v, "idx {i}");
        }
        assert_eq!(b.null_count(), pattern.iter().filter(|v| !**v).count());
    }

    #[test]
    fn all_set_has_no_nulls() {
        for &len in &[0usize, 1, 7, 8, 9, 63, 64, 65, 123] {
            let b = Bitmap::all_set(len);
            assert_eq!(b.len, len);
            for i in 0..len {
                assert!(b.is_valid(i), "len={len} idx={i}");
            }
            assert_eq!(b.null_count(), 0);
        }
    }

    #[test]
    fn roundtrip_bytes() {
        let mut b = Bitmap::with_capacity(0);
        for v in [true, false, true, false, false, true, true, true, true, false] {
            b.push(v);
        }
        let len = b.len;
        let bytes = b.as_bytes().to_vec();
        let null_count = b.null_count;
        let back = Bitmap::from_bytes(bytes, len);
        assert_eq!(back.null_count, null_count);
        for i in 0..len {
            assert_eq!(back.is_valid(i), b.is_valid(i));
        }
    }
}
