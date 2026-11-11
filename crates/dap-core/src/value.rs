//! Typed analytic value model.
//!
//! `LogicalType` is what query authors reason about; `PhysicalType` is the in-memory encoding
//! used by the vectorized runtime and the `.derc` format. One logical type may map to several
//! physical encodings depending on segment statistics.
//!
//! `Value` is a scalar reference form for the **correctness oracle** and for metadata (zone
//! maps, catalog defaults). Vectorized operators do not store rows as `Value` instances; they
//! operate on typed column buffers (see `dap-runtime`).

use serde::{Deserialize, Serialize};

use crate::error::{DapError, DapResult};
use crate::uid::Uid;

/// Logical type exposed to the query language and catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogicalType {
    Bool,
    I32,
    I64,
    F64,
    /// Fixed-point decimal with `(precision, scale)`.
    Decimal { precision: u8, scale: u8 },
    /// UTF-8 variable-length string.
    Utf8,
    /// DHIS2-style identifier (11 chars).
    Uid,
    /// Date / time at millisecond resolution, UTC.
    Timestamp,
    /// DHIS2 reporting period, stored as its canonical ISO code (e.g. "202601", "2026W03").
    Period,
}

/// Physical encoding selected by the storage writer per page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhysicalType {
    Bool,
    I32,
    I64,
    F64,
    /// Pre-scaled decimal stored as i128 (fits every DHIS2 precision).
    Decimal128,
    Utf8,
    Timestamp,
    /// Dictionary-encoded indices into a column-local dictionary.
    DictIndex32,
}

impl LogicalType {
    pub fn default_physical(&self) -> PhysicalType {
        match self {
            LogicalType::Bool => PhysicalType::Bool,
            LogicalType::I32 => PhysicalType::I32,
            LogicalType::I64 => PhysicalType::I64,
            LogicalType::F64 => PhysicalType::F64,
            LogicalType::Decimal { .. } => PhysicalType::Decimal128,
            LogicalType::Utf8 => PhysicalType::Utf8,
            LogicalType::Uid => PhysicalType::Utf8,
            LogicalType::Timestamp => PhysicalType::Timestamp,
            LogicalType::Period => PhysicalType::Utf8,
        }
    }

    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            LogicalType::I32 | LogicalType::I64 | LogicalType::F64 | LogicalType::Decimal { .. }
        )
    }
}

/// Scalar reference form used by the correctness oracle. A `None` represents SQL NULL.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    Decimal128 { value: i128, scale: u8 },
    Str(String),
    Uid(Uid),
    Timestamp(i64), // unix millis
    Period(String),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn logical_type(&self) -> Option<LogicalType> {
        Some(match self {
            Value::Null => return None,
            Value::Bool(_) => LogicalType::Bool,
            Value::I64(_) => LogicalType::I64,
            Value::F64(_) => LogicalType::F64,
            Value::Decimal128 { scale, .. } => LogicalType::Decimal {
                precision: 38,
                scale: *scale,
            },
            Value::Str(_) => LogicalType::Utf8,
            Value::Uid(_) => LogicalType::Uid,
            Value::Timestamp(_) => LogicalType::Timestamp,
            Value::Period(_) => LogicalType::Period,
        })
    }

    pub fn as_i64(&self) -> DapResult<i64> {
        match self {
            Value::I64(v) => Ok(*v),
            Value::Bool(b) => Ok(i64::from(*b)),
            Value::Decimal128 { value, scale } if *scale == 0 => i64::try_from(*value)
                .map_err(|_| DapError::invalid_value("decimal does not fit i64")),
            _ => Err(DapError::TypeError {
                expected: "i64".into(),
                found: format!("{:?}", self),
            }),
        }
    }

    pub fn as_f64(&self) -> DapResult<f64> {
        match self {
            Value::F64(v) => Ok(*v),
            Value::I64(v) => Ok(*v as f64),
            Value::Bool(b) => Ok(f64::from(*b)),
            Value::Decimal128 { value, scale } => {
                let divisor = 10f64.powi(*scale as i32);
                Ok(*value as f64 / divisor)
            }
            _ => Err(DapError::TypeError {
                expected: "f64".into(),
                found: format!("{:?}", self),
            }),
        }
    }

    pub fn as_str(&self) -> DapResult<&str> {
        match self {
            Value::Str(s) => Ok(s.as_str()),
            Value::Uid(u) => Ok(u.as_str()),
            Value::Period(s) => Ok(s.as_str()),
            _ => Err(DapError::TypeError {
                expected: "utf8".into(),
                found: format!("{:?}", self),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_coercion_rules() {
        assert_eq!(Value::I64(42).as_f64().unwrap(), 42.0);
        assert_eq!(Value::Bool(true).as_i64().unwrap(), 1);
        assert_eq!(
            Value::Decimal128 { value: 12345, scale: 2 }.as_f64().unwrap(),
            123.45
        );
        assert!(Value::Str("nope".into()).as_i64().is_err());
    }

    #[test]
    fn logical_type_for_values() {
        assert_eq!(Value::I64(1).logical_type(), Some(LogicalType::I64));
        assert_eq!(Value::Null.logical_type(), None);
    }

    #[test]
    fn default_physical_mapping() {
        assert_eq!(
            LogicalType::Period.default_physical(),
            PhysicalType::Utf8
        );
        assert_eq!(
            LogicalType::Decimal { precision: 18, scale: 2 }.default_physical(),
            PhysicalType::Decimal128
        );
    }
}
