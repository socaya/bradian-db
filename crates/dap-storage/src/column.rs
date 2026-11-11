//! Typed physical column buffers.
//!
//! A `ColumnBuffer` is a dense, validity-aware column of a single physical type. The storage
//! writer consumes these to produce `.derc` pages; the reader produces them back on scan. The
//! vectorized runtime (`dap-runtime::ColumnBatch`) wraps these with selection-vector and
//! operator-oriented metadata.

use dap_core::{DapError, DapResult, LogicalType, PhysicalType, Value};
use serde::{Deserialize, Serialize};

use crate::bitmap::Bitmap;

/// A single-column, typed, dense buffer with explicit validity.
///
/// Null values occupy a slot in the underlying storage; the bitmap says whether that slot
/// is valid. This matches Arrow's "storage of invalid value is unspecified" convention and
/// keeps indices stable across Scan/Filter/Project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ColumnBuffer {
    Bool { data: Vec<u8>, validity: Bitmap, len: usize }, // bool packed, LSB-first per byte
    I32 { data: Vec<i32>, validity: Bitmap },
    I64 { data: Vec<i64>, validity: Bitmap },
    F64 { data: Vec<f64>, validity: Bitmap },
    /// Pre-scaled decimal values. `scale` is shared for the column.
    Decimal128 { data: Vec<i128>, validity: Bitmap, scale: u8 },
    Utf8 { offsets: Vec<u32>, data: Vec<u8>, validity: Bitmap },
    /// Millis since Unix epoch, UTC.
    Timestamp { data: Vec<i64>, validity: Bitmap },
    /// Dictionary-encoded column. `indices` references `dictionary`. Dictionary is a
    /// contiguous Utf8 blob keyed by `dict_offsets`.
    DictIndex32 {
        indices: Vec<u32>,
        validity: Bitmap,
        dict_offsets: Vec<u32>,
        dict_data: Vec<u8>,
    },
}

/// Scalar value drawn from a `ColumnBuffer`, used by the scalar oracle in tests and by storage
/// writers when they accept `Value` rather than a specialised typed path.
pub type ColumnValue = Value;

impl ColumnBuffer {
    pub fn physical_type(&self) -> PhysicalType {
        match self {
            ColumnBuffer::Bool { .. } => PhysicalType::Bool,
            ColumnBuffer::I32 { .. } => PhysicalType::I32,
            ColumnBuffer::I64 { .. } => PhysicalType::I64,
            ColumnBuffer::F64 { .. } => PhysicalType::F64,
            ColumnBuffer::Decimal128 { .. } => PhysicalType::Decimal128,
            ColumnBuffer::Utf8 { .. } => PhysicalType::Utf8,
            ColumnBuffer::Timestamp { .. } => PhysicalType::Timestamp,
            ColumnBuffer::DictIndex32 { .. } => PhysicalType::DictIndex32,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            ColumnBuffer::Bool { len, .. } => *len,
            ColumnBuffer::I32 { data, .. } => data.len(),
            ColumnBuffer::I64 { data, .. } => data.len(),
            ColumnBuffer::F64 { data, .. } => data.len(),
            ColumnBuffer::Decimal128 { data, .. } => data.len(),
            ColumnBuffer::Utf8 { offsets, .. } => offsets.len().saturating_sub(1),
            ColumnBuffer::Timestamp { data, .. } => data.len(),
            ColumnBuffer::DictIndex32 { indices, .. } => indices.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn null_count(&self) -> usize {
        self.validity().null_count()
    }

    pub fn validity(&self) -> &Bitmap {
        match self {
            ColumnBuffer::Bool { validity, .. }
            | ColumnBuffer::I32 { validity, .. }
            | ColumnBuffer::I64 { validity, .. }
            | ColumnBuffer::F64 { validity, .. }
            | ColumnBuffer::Decimal128 { validity, .. }
            | ColumnBuffer::Utf8 { validity, .. }
            | ColumnBuffer::Timestamp { validity, .. }
            | ColumnBuffer::DictIndex32 { validity, .. } => validity,
        }
    }

    /// Read a row as a boxed `Value`. Not used on hot paths — see runtime typed operators.
    pub fn value_at(&self, idx: usize) -> DapResult<Value> {
        if idx >= self.len() {
            return Err(DapError::invariant(format!(
                "value_at {idx} out of range (len={})",
                self.len()
            )));
        }
        if !self.validity().is_valid(idx) {
            return Ok(Value::Null);
        }
        Ok(match self {
            ColumnBuffer::Bool { data, .. } => Value::Bool((data[idx / 8] >> (idx % 8)) & 1 == 1),
            ColumnBuffer::I32 { data, .. } => Value::I64(data[idx] as i64),
            ColumnBuffer::I64 { data, .. } => Value::I64(data[idx]),
            ColumnBuffer::F64 { data, .. } => Value::F64(data[idx]),
            ColumnBuffer::Decimal128 { data, scale, .. } => Value::Decimal128 {
                value: data[idx],
                scale: *scale,
            },
            ColumnBuffer::Utf8 { offsets, data, .. } => {
                let s = offsets[idx] as usize;
                let e = offsets[idx + 1] as usize;
                Value::Str(String::from_utf8(data[s..e].to_vec()).map_err(|e| {
                    DapError::corruption(format!("non-utf8 in column: {e}"))
                })?)
            }
            ColumnBuffer::Timestamp { data, .. } => Value::Timestamp(data[idx]),
            ColumnBuffer::DictIndex32 {
                indices,
                dict_offsets,
                dict_data,
                ..
            } => {
                let i = indices[idx] as usize;
                if i + 1 >= dict_offsets.len() {
                    return Err(DapError::corruption("dict index out of range"));
                }
                let s = dict_offsets[i] as usize;
                let e = dict_offsets[i + 1] as usize;
                Value::Str(String::from_utf8(dict_data[s..e].to_vec()).map_err(|e| {
                    DapError::corruption(format!("non-utf8 in dictionary: {e}"))
                })?)
            }
        })
    }
}

/// Typed column builder producing a `ColumnBuffer`.
#[derive(Debug)]
pub enum ColumnBuilder {
    Bool(BoolBuilder),
    I32(PrimitiveBuilder<i32>),
    I64(PrimitiveBuilder<i64>),
    F64(PrimitiveBuilder<f64>),
    Decimal128(DecimalBuilder),
    Utf8(Utf8Builder),
    Timestamp(PrimitiveBuilder<i64>),
    /// String-dictionary builder. Build order is append-only; `finish()` yields `DictIndex32`.
    DictUtf8(DictUtf8Builder),
}

impl ColumnBuilder {
    pub fn new(logical: LogicalType) -> Self {
        match logical {
            LogicalType::Bool => ColumnBuilder::Bool(BoolBuilder::default()),
            LogicalType::I32 => ColumnBuilder::I32(PrimitiveBuilder::default()),
            LogicalType::I64 => ColumnBuilder::I64(PrimitiveBuilder::default()),
            LogicalType::F64 => ColumnBuilder::F64(PrimitiveBuilder::default()),
            LogicalType::Decimal { scale, .. } => {
                ColumnBuilder::Decimal128(DecimalBuilder::new(scale))
            }
            LogicalType::Utf8 => ColumnBuilder::Utf8(Utf8Builder::default()),
            LogicalType::Uid => ColumnBuilder::Utf8(Utf8Builder::default()),
            LogicalType::Period => ColumnBuilder::Utf8(Utf8Builder::default()),
            LogicalType::Timestamp => ColumnBuilder::Timestamp(PrimitiveBuilder::default()),
        }
    }

    pub fn new_dict(_logical: LogicalType) -> Self {
        ColumnBuilder::DictUtf8(DictUtf8Builder::default())
    }

    pub fn append_value(&mut self, value: &Value) -> DapResult<()> {
        if value.is_null() {
            self.append_null();
            return Ok(());
        }
        match (self, value) {
            (ColumnBuilder::Bool(b), Value::Bool(v)) => {
                b.append(*v);
                Ok(())
            }
            (ColumnBuilder::I32(b), Value::I64(v)) => {
                let v = i32::try_from(*v).map_err(|_| DapError::invalid_value("i32 overflow"))?;
                b.append(v);
                Ok(())
            }
            (ColumnBuilder::I64(b), Value::I64(v)) => {
                b.append(*v);
                Ok(())
            }
            (ColumnBuilder::F64(b), Value::F64(v)) => {
                b.append(*v);
                Ok(())
            }
            (ColumnBuilder::F64(b), Value::I64(v)) => {
                b.append(*v as f64);
                Ok(())
            }
            (ColumnBuilder::Decimal128(b), Value::Decimal128 { value, scale }) => {
                if *scale != b.scale {
                    return Err(DapError::invalid_value(format!(
                        "decimal scale mismatch: col={} value={}",
                        b.scale, scale
                    )));
                }
                b.append(*value);
                Ok(())
            }
            (ColumnBuilder::Utf8(b), Value::Str(s)) => {
                b.append(s.as_bytes());
                Ok(())
            }
            (ColumnBuilder::Utf8(b), Value::Uid(u)) => {
                b.append(u.as_ref().as_bytes());
                Ok(())
            }
            (ColumnBuilder::Utf8(b), Value::Period(p)) => {
                b.append(p.as_bytes());
                Ok(())
            }
            (ColumnBuilder::Timestamp(b), Value::Timestamp(t)) => {
                b.append(*t);
                Ok(())
            }
            (ColumnBuilder::DictUtf8(b), Value::Str(s)) => {
                b.append_str(s);
                Ok(())
            }
            (ColumnBuilder::DictUtf8(b), Value::Uid(u)) => {
                b.append_str(u.as_ref());
                Ok(())
            }
            (ColumnBuilder::DictUtf8(b), Value::Period(p)) => {
                b.append_str(p);
                Ok(())
            }
            (builder, value) => Err(DapError::TypeError {
                expected: format!("{builder:?}"),
                found: format!("{value:?}"),
            }),
        }
    }

    pub fn append_null(&mut self) {
        match self {
            ColumnBuilder::Bool(b) => b.append_null(),
            ColumnBuilder::I32(b) => b.append_null(0),
            ColumnBuilder::I64(b) => b.append_null(0),
            ColumnBuilder::F64(b) => b.append_null(0.0),
            ColumnBuilder::Decimal128(b) => b.append_null(),
            ColumnBuilder::Utf8(b) => b.append_null(),
            ColumnBuilder::Timestamp(b) => b.append_null(0),
            ColumnBuilder::DictUtf8(b) => b.append_null(),
        }
    }

    pub fn finish(self) -> ColumnBuffer {
        match self {
            ColumnBuilder::Bool(b) => ColumnBuffer::Bool {
                data: b.bits,
                validity: b.validity,
                len: b.len,
            },
            ColumnBuilder::I32(b) => ColumnBuffer::I32 {
                data: b.data,
                validity: b.validity,
            },
            ColumnBuilder::I64(b) => ColumnBuffer::I64 {
                data: b.data,
                validity: b.validity,
            },
            ColumnBuilder::F64(b) => ColumnBuffer::F64 {
                data: b.data,
                validity: b.validity,
            },
            ColumnBuilder::Decimal128(b) => ColumnBuffer::Decimal128 {
                data: b.data,
                validity: b.validity,
                scale: b.scale,
            },
            ColumnBuilder::Utf8(b) => ColumnBuffer::Utf8 {
                offsets: b.offsets,
                data: b.data,
                validity: b.validity,
            },
            ColumnBuilder::Timestamp(b) => ColumnBuffer::Timestamp {
                data: b.data,
                validity: b.validity,
            },
            ColumnBuilder::DictUtf8(b) => b.finish(),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            ColumnBuilder::Bool(b) => b.len,
            ColumnBuilder::I32(b) => b.data.len(),
            ColumnBuilder::I64(b) => b.data.len(),
            ColumnBuilder::F64(b) => b.data.len(),
            ColumnBuilder::Decimal128(b) => b.data.len(),
            ColumnBuilder::Utf8(b) => b.offsets.len().saturating_sub(1),
            ColumnBuilder::Timestamp(b) => b.data.len(),
            ColumnBuilder::DictUtf8(b) => b.indices.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Default)]
pub struct BoolBuilder {
    bits: Vec<u8>,
    validity: Bitmap,
    len: usize,
}
impl BoolBuilder {
    pub fn append(&mut self, v: bool) {
        self.push_bit(v);
        self.validity.push(true);
        self.len += 1;
    }
    pub fn append_null(&mut self) {
        self.push_bit(false);
        self.validity.push(false);
        self.len += 1;
    }
    fn push_bit(&mut self, v: bool) {
        let i = self.len;
        let byte = i / 8;
        if byte >= self.bits.len() {
            self.bits.push(0);
        }
        if v {
            self.bits[byte] |= 1 << (i % 8);
        }
    }
}

#[derive(Debug, Default)]
pub struct PrimitiveBuilder<T: Copy> {
    data: Vec<T>,
    validity: Bitmap,
}
impl<T: Copy> PrimitiveBuilder<T> {
    pub fn append(&mut self, v: T) {
        self.data.push(v);
        self.validity.push(true);
    }
    pub fn append_null(&mut self, placeholder: T) {
        self.data.push(placeholder);
        self.validity.push(false);
    }
}

#[derive(Debug)]
pub struct DecimalBuilder {
    data: Vec<i128>,
    validity: Bitmap,
    scale: u8,
}
impl DecimalBuilder {
    pub fn new(scale: u8) -> Self {
        Self {
            data: Vec::new(),
            validity: Bitmap::default(),
            scale,
        }
    }
    pub fn append(&mut self, v: i128) {
        self.data.push(v);
        self.validity.push(true);
    }
    pub fn append_null(&mut self) {
        self.data.push(0);
        self.validity.push(false);
    }
}

#[derive(Debug)]
pub struct Utf8Builder {
    offsets: Vec<u32>,
    data: Vec<u8>,
    validity: Bitmap,
}
impl Default for Utf8Builder {
    fn default() -> Self {
        Self {
            offsets: vec![0],
            data: Vec::new(),
            validity: Bitmap::default(),
        }
    }
}
impl Utf8Builder {
    pub fn append(&mut self, bytes: &[u8]) {
        self.data.extend_from_slice(bytes);
        self.offsets.push(self.data.len() as u32);
        self.validity.push(true);
    }
    pub fn append_null(&mut self) {
        self.offsets.push(self.data.len() as u32);
        self.validity.push(false);
    }
}

#[derive(Debug, Default)]
pub struct DictUtf8Builder {
    indices: Vec<u32>,
    validity: Bitmap,
    dict: ahash::AHashMap<String, u32>,
    strings: Vec<String>,
}
impl DictUtf8Builder {
    pub fn append_str(&mut self, s: &str) {
        let idx = match self.dict.get(s) {
            Some(&i) => i,
            None => {
                let i = self.strings.len() as u32;
                self.strings.push(s.to_string());
                self.dict.insert(s.to_string(), i);
                i
            }
        };
        self.indices.push(idx);
        self.validity.push(true);
    }
    pub fn append_null(&mut self) {
        self.indices.push(u32::MAX);
        self.validity.push(false);
    }
    pub fn finish(self) -> ColumnBuffer {
        let mut dict_offsets = Vec::with_capacity(self.strings.len() + 1);
        let mut dict_data = Vec::new();
        dict_offsets.push(0u32);
        for s in &self.strings {
            dict_data.extend_from_slice(s.as_bytes());
            dict_offsets.push(dict_data.len() as u32);
        }
        ColumnBuffer::DictIndex32 {
            indices: self.indices,
            validity: self.validity,
            dict_offsets,
            dict_data,
        }
    }
    pub fn cardinality(&self) -> usize {
        self.strings.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dap_core::LogicalType;

    #[test]
    fn i64_roundtrip() {
        let mut b = ColumnBuilder::new(LogicalType::I64);
        b.append_value(&Value::I64(1)).unwrap();
        b.append_value(&Value::Null).unwrap();
        b.append_value(&Value::I64(3)).unwrap();
        let buf = b.finish();
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.null_count(), 1);
        assert_eq!(buf.value_at(0).unwrap(), Value::I64(1));
        assert_eq!(buf.value_at(1).unwrap(), Value::Null);
        assert_eq!(buf.value_at(2).unwrap(), Value::I64(3));
    }

    #[test]
    fn utf8_variable_widths() {
        let mut b = ColumnBuilder::new(LogicalType::Utf8);
        for s in ["hello", "", "world of DER"] {
            b.append_value(&Value::Str(s.into())).unwrap();
        }
        let buf = b.finish();
        assert_eq!(buf.value_at(0).unwrap(), Value::Str("hello".into()));
        assert_eq!(buf.value_at(1).unwrap(), Value::Str("".into()));
        assert_eq!(buf.value_at(2).unwrap(), Value::Str("world of DER".into()));
    }

    #[test]
    fn dict_builder_collapses_duplicates() {
        let mut b = ColumnBuilder::new_dict(LogicalType::Utf8);
        for s in ["a", "b", "a", "a", "c", "b"] {
            b.append_value(&Value::Str(s.into())).unwrap();
        }
        let buf = b.finish();
        if let ColumnBuffer::DictIndex32 { dict_offsets, indices, .. } = &buf {
            assert_eq!(dict_offsets.len() - 1, 3, "cardinality");
            assert_eq!(indices.len(), 6);
        } else {
            panic!("expected DictIndex32");
        }
        assert_eq!(buf.value_at(0).unwrap(), Value::Str("a".into()));
        assert_eq!(buf.value_at(3).unwrap(), Value::Str("a".into()));
        assert_eq!(buf.value_at(4).unwrap(), Value::Str("c".into()));
    }

    #[test]
    fn bool_packed_roundtrip() {
        let mut b = ColumnBuilder::new(LogicalType::Bool);
        let pattern = [true, false, true, true, false, false, false, true, true];
        for v in pattern {
            b.append_value(&Value::Bool(v)).unwrap();
        }
        let buf = b.finish();
        for (i, v) in pattern.iter().enumerate() {
            assert_eq!(buf.value_at(i).unwrap(), Value::Bool(*v));
        }
    }
}
