//! Canonical change envelope. Matches the shape in spec §7.1 (figure).
//!
//! `payload` is a JSON object whose shape depends on the `object_type`. For aggregate
//! facts: `{ "ou": Uid, "period": "202601", "de": Uid, "coc": Uid, "value": number | null }`.

use dap_core::{DapError, DapResult, Uid, Value};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Operation {
    Upsert,
    Delete,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Change {
    #[serde(rename = "changeId")]
    pub change_id: String,
    #[serde(rename = "siteId")]
    pub site_id: Option<String>,
    #[serde(rename = "objectType")]
    pub object_type: String,
    #[serde(rename = "objectUid")]
    pub object_uid: Uid,
    #[serde(rename = "sourceVersion")]
    pub source_version: u64,
    pub operation: Operation,
    #[serde(rename = "occurredAt")]
    pub occurred_at: String,
    #[serde(rename = "metadataRevision")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_revision: Option<String>,
    #[serde(rename = "payloadHash")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_hash: Option<String>,
    pub payload: serde_json::Value,
}

impl Change {
    /// Convert the payload into a typed row ordered by the target schema. The payload is a
    /// JSON object whose keys match schema column names.
    pub fn row_for_schema(&self, schema: &dap_core::Schema) -> DapResult<Vec<Value>> {
        let obj = self
            .payload
            .as_object()
            .ok_or_else(|| DapError::invalid_value("change payload must be a JSON object"))?;
        let mut row = Vec::with_capacity(schema.len());
        for col in &schema.columns {
            let v = obj
                .get(&col.name)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            row.push(coerce(v, col.logical_type)?);
        }
        Ok(row)
    }
}

fn coerce(v: serde_json::Value, ty: dap_core::LogicalType) -> DapResult<Value> {
    use dap_core::LogicalType as T;
    use serde_json::Value as J;
    if v.is_null() {
        return Ok(Value::Null);
    }
    Ok(match (ty, v) {
        (T::Bool, J::Bool(b)) => Value::Bool(b),
        (T::I32 | T::I64, J::Number(n)) => Value::I64(
            n.as_i64()
                .ok_or_else(|| DapError::invalid_value("expected integer"))?,
        ),
        (T::F64, J::Number(n)) => Value::F64(
            n.as_f64()
                .ok_or_else(|| DapError::invalid_value("expected number"))?,
        ),
        (T::Decimal { scale, .. }, J::String(s)) => {
            let value = parse_decimal(&s, scale)?;
            Value::Decimal128 { value, scale }
        }
        (T::Decimal { scale, .. }, J::Number(n)) => {
            let f = n
                .as_f64()
                .ok_or_else(|| DapError::invalid_value("decimal number"))?;
            let factor = 10f64.powi(scale as i32);
            Value::Decimal128 {
                value: (f * factor).round() as i128,
                scale,
            }
        }
        (T::Utf8, J::String(s)) => Value::Str(s),
        (T::Uid, J::String(s)) => {
            let u = dap_core::Uid::new(s)?;
            Value::Str(u.as_str().to_string())
        }
        (T::Timestamp, J::Number(n)) => Value::Timestamp(
            n.as_i64()
                .ok_or_else(|| DapError::invalid_value("timestamp i64"))?,
        ),
        (T::Period, J::String(s)) => {
            let _ = dap_core::Period::parse(&s)?;
            Value::Str(s)
        }
        (expected, got) => {
            return Err(DapError::TypeError {
                expected: format!("{expected:?}"),
                found: format!("{got}"),
            })
        }
    })
}

fn parse_decimal(s: &str, scale: u8) -> DapResult<i128> {
    let mut parts = s.splitn(2, '.');
    let int_part = parts.next().unwrap_or("0");
    let frac_part = parts.next().unwrap_or("");
    let negative = int_part.starts_with('-');
    let int_digits: String = int_part.chars().filter(|c| c.is_ascii_digit()).collect();
    let mut frac_digits: String = frac_part.chars().filter(|c| c.is_ascii_digit()).collect();
    if frac_digits.len() > scale as usize {
        frac_digits.truncate(scale as usize);
    }
    while frac_digits.len() < scale as usize {
        frac_digits.push('0');
    }
    let combined = format!("{}{}", int_digits, frac_digits);
    let value: i128 = combined.parse().map_err(|e: std::num::ParseIntError| {
        DapError::invalid_value(format!("decimal parse: {e}"))
    })?;
    Ok(if negative { -value } else { value })
}
