//! Dictionary codec: indices + dictionary payload.
//!
//! Layout:
//!   u32 n_rows
//!   u32 n_dict
//!   u32[n_rows] indices
//!   u32[n_dict + 1] dict offsets
//!   u8[] dict bytes

use bradian_core::{BradianError, BradianResult};

use crate::column::ColumnBuffer;

pub fn encode(buf: &ColumnBuffer) -> BradianResult<Vec<u8>> {
    match buf {
        ColumnBuffer::DictIndex32 {
            indices,
            dict_offsets,
            dict_data,
            ..
        } => {
            let n_rows = indices.len() as u32;
            let n_dict = (dict_offsets.len() - 1) as u32;
            let mut out = Vec::with_capacity(
                8 + indices.len() * 4 + dict_offsets.len() * 4 + dict_data.len(),
            );
            out.extend_from_slice(&n_rows.to_le_bytes());
            out.extend_from_slice(&n_dict.to_le_bytes());
            for idx in indices {
                out.extend_from_slice(&idx.to_le_bytes());
            }
            for off in dict_offsets.iter() {
                out.extend_from_slice(&off.to_le_bytes());
            }
            out.extend_from_slice(dict_data.as_slice());
            Ok(out)
        }
        other => Err(BradianError::unsupported(format!(
            "dict codec cannot encode {:?}",
            other.physical_type()
        ))),
    }
}

/// ADR 0083 §3.3 — v1.4 column-dict page encoding. The dict
/// bytes live once in the column-dict region; each page writes
/// only (n_rows, n_dict_of_column, indices). `n_dict_of_column`
/// is stored for cross-checking against the column-dict entry
/// the reader loads from the region.
pub fn encode_indices_only(buf: &ColumnBuffer) -> BradianResult<Vec<u8>> {
    match buf {
        ColumnBuffer::DictIndex32 { indices, dict_offsets, .. } => {
            let n_rows = indices.len() as u32;
            let n_dict = (dict_offsets.len() - 1) as u32;
            let mut out = Vec::with_capacity(8 + indices.len() * 4);
            out.extend_from_slice(&n_rows.to_le_bytes());
            out.extend_from_slice(&n_dict.to_le_bytes());
            for idx in indices {
                out.extend_from_slice(&idx.to_le_bytes());
            }
            Ok(out)
        }
        other => Err(BradianError::unsupported(format!(
            "dict codec (indices-only) cannot encode {:?}",
            other.physical_type()
        ))),
    }
}

/// ADR 0083 §3.4 — v1.4 column-dict page decoding. Returns the
/// indices and the declared `n_dict_of_column`; the caller must
/// cross-check against the column-dict region's actual size.
pub fn decode_indices_only(bytes: &[u8]) -> BradianResult<(Vec<u32>, u32)> {
    if bytes.len() < 8 {
        return Err(BradianError::corruption("dict page (indices-only) too short"));
    }
    let n_rows = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    let n_dict = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let idx_end = 8 + n_rows * 4;
    if bytes.len() < idx_end {
        return Err(BradianError::corruption("dict page (indices-only) truncated"));
    }
    if bytes.len() != idx_end {
        return Err(BradianError::corruption(
            "dict page (indices-only) has trailing bytes",
        ));
    }
    let mut indices = Vec::with_capacity(n_rows);
    for chunk in bytes[8..idx_end].as_chunks::<4>().0.iter() {
        indices.push(u32::from_le_bytes(*chunk));
    }
    Ok((indices, n_dict))
}

pub fn decode(bytes: &[u8]) -> BradianResult<(Vec<u32>, Vec<u32>, Vec<u8>)> {
    if bytes.len() < 8 {
        return Err(BradianError::corruption("dict page too short"));
    }
    let n_rows = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    let n_dict = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;

    let idx_end = 8 + n_rows * 4;
    let off_end = idx_end + (n_dict + 1) * 4;
    if bytes.len() < off_end {
        return Err(BradianError::corruption("dict page truncated"));
    }

    let mut indices = Vec::with_capacity(n_rows);
    for chunk in bytes[8..idx_end].as_chunks::<4>().0.iter() {
        indices.push(u32::from_le_bytes(*chunk));
    }
    let mut dict_offsets = Vec::with_capacity(n_dict + 1);
    for chunk in bytes[idx_end..off_end].as_chunks::<4>().0.iter() {
        dict_offsets.push(u32::from_le_bytes(*chunk));
    }
    let dict_data = bytes[off_end..].to_vec();
    let expected = *dict_offsets.last().unwrap_or(&0) as usize;
    if dict_data.len() != expected {
        return Err(BradianError::corruption(format!(
            "dict data length {} does not match last offset {}",
            dict_data.len(),
            expected
        )));
    }
    // Validate indices do not escape dictionary (except sentinel u32::MAX for nulls).
    for (i, idx) in indices.iter().enumerate() {
        if *idx != u32::MAX && (*idx as usize) >= n_dict {
            return Err(BradianError::corruption(format!(
                "dict index {idx} at row {i} out of range (n_dict={n_dict})"
            )));
        }
    }
    Ok((indices, dict_offsets, dict_data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::column::ColumnBuilder;
    use bradian_core::{LogicalType, Value};

    #[test]
    fn roundtrip() {
        let mut b = ColumnBuilder::new_dict(LogicalType::Utf8);
        for s in ["ouA", "ouB", "ouA", "ouC", "ouB"] {
            b.append_value(&Value::Str(s.into())).unwrap();
        }
        let buf = b.finish();
        let enc = encode(&buf).unwrap();
        let (indices, offsets, data) = decode(&enc).unwrap();
        assert_eq!(indices.len(), 5);
        assert_eq!(offsets.len() - 1, 3);
        assert_eq!(data, b"ouAouBouC");
    }

    #[test]
    fn nulls_as_sentinel() {
        let mut b = ColumnBuilder::new_dict(LogicalType::Utf8);
        b.append_value(&Value::Str("ouA".into())).unwrap();
        b.append_value(&Value::Null).unwrap();
        b.append_value(&Value::Str("ouA".into())).unwrap();
        let buf = b.finish();
        let enc = encode(&buf).unwrap();
        let (indices, _, _) = decode(&enc).unwrap();
        assert_eq!(indices[1], u32::MAX, "null encoded as sentinel");
        let _ = &buf; // sink
    }

    #[test]
    fn corrupt_index_rejected() {
        // Build a valid dict page then flip one index byte.
        let mut b = ColumnBuilder::new_dict(LogicalType::Utf8);
        b.append_value(&Value::Str("a".into())).unwrap();
        let buf = b.finish();
        let mut enc = encode(&buf).unwrap();
        enc[8] = 0xFE;
        enc[9] = 0xFF;
        enc[10] = 0x00;
        enc[11] = 0x00;
        // 0x0000FFFE -> within u32::MAX sentinel? No; 0xFFFFFFFE is sentinel check; we use 0x0000FFFE.
        assert!(decode(&enc).is_err());
    }
}
