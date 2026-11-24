//! ADR 0304 — Bradian-native batch IPC encoding.
//!
//! A deliberate stepping stone: ships the "batch
//! round-trip" acceptance for P1-4 slice 4 without taking
//! the full `arrow` crate dependency. The wire format is
//! `BRCH\x01` + a bincode-serialised `IpcRecordBatch`.
//!
//! This is NOT Apache Arrow IPC. A consumer expecting the
//! Apache wire format cannot read these bytes. A later
//! slice can introduce an actual Arrow adapter under a
//! feature flag once the dependency is approved.

use bradian_core::{BradianError, BradianResult};
use serde::{Deserialize, Serialize};

const MAGIC: &[u8; 5] = b"BRCH\x01";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IpcValueType { Bool, I64, F64, Str }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpcColumn {
    pub name: String,
    pub ty:   IpcValueType,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum IpcValue {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    Str(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IpcRecordBatch {
    pub schema: Vec<IpcColumn>,
    pub rows:   Vec<Vec<IpcValue>>,
}

pub fn write_ipc(batch: &IpcRecordBatch) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + batch.rows.len() * 16);
    out.extend_from_slice(MAGIC);
    let body = bincode::serialize(batch).expect("bincode serialize ipc batch");
    out.extend_from_slice(&body);
    out
}

pub fn read_ipc(bytes: &[u8]) -> BradianResult<IpcRecordBatch> {
    if bytes.len() < MAGIC.len() || &bytes[..MAGIC.len()] != MAGIC {
        return Err(BradianError::invalid_value(
            "bradian IPC batch: missing or wrong BRCH\\x01 magic header"));
    }
    let batch: IpcRecordBatch = bincode::deserialize(&bytes[MAGIC.len()..])
        .map_err(|e| BradianError::Serialization(format!("ipc decode: {e}")))?;
    validate_shape(&batch)?;
    Ok(batch)
}

fn validate_shape(batch: &IpcRecordBatch) -> BradianResult<()> {
    let width = batch.schema.len();
    for (i, row) in batch.rows.iter().enumerate() {
        if row.len() != width {
            return Err(BradianError::schema_mismatch(format!(
                "ipc row {i} has {} fields, schema has {}", row.len(), width)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> IpcRecordBatch {
        IpcRecordBatch {
            schema: vec![
                IpcColumn { name: "id".into(),    ty: IpcValueType::I64 },
                IpcColumn { name: "ratio".into(), ty: IpcValueType::F64 },
                IpcColumn { name: "live".into(),  ty: IpcValueType::Bool },
                IpcColumn { name: "label".into(), ty: IpcValueType::Str },
            ],
            rows: vec![
                vec![IpcValue::I64(1), IpcValue::F64(0.5),  IpcValue::Bool(true),  IpcValue::Str("alpha".into())],
                vec![IpcValue::I64(2), IpcValue::F64(1.25), IpcValue::Bool(false), IpcValue::Null],
            ],
        }
    }

    #[test]
    fn empty_batch_round_trips() {
        let b = IpcRecordBatch { schema: vec![], rows: vec![] };
        let bytes = write_ipc(&b);
        assert_eq!(read_ipc(&bytes).unwrap(), b);
    }

    #[test]
    fn typed_batch_round_trips_exactly() {
        let b = sample();
        let bytes = write_ipc(&b);
        assert_eq!(read_ipc(&bytes).unwrap(), b);
    }

    #[test]
    fn missing_magic_header_is_rejected() {
        let bytes = bincode::serialize(&sample()).unwrap();
        let err = read_ipc(&bytes).unwrap_err();
        assert!(format!("{err}").contains("BRCH"));
    }

    #[test]
    fn width_mismatch_in_body_is_rejected_on_read() {
        let mut b = sample();
        b.rows[0].pop();   // intentionally break row 0's width
        let bytes = write_ipc(&b);
        let err = read_ipc(&bytes).unwrap_err();
        assert!(format!("{err}").contains("row 0"),
            "expected row-0 width mismatch, got: {err}");
    }
}
