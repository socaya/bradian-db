//! G32 version-tagged row payloads, integrated with immutable-schema SQL by G35.
//!
//! The existing BRRW v1 payload remains unchanged. A BRVE envelope binds a
//! physical row to one explicit schema epoch and its immutable definition.
//! This codec does not activate DDL, update the G11 catalog or modify G22 WAL.

use std::collections::BTreeMap;
use crc32fast::hash;
use thiserror::Error;
use crate::{Table, TableId};
use crate::typed_table::{decode_row, encode_row, TableError, Value};

const MAGIC: &[u8; 4] = b"BRVE";
const VERSION: u8 = 1;
const HEADER: usize = 4 + 1 + 8 + 8 + 4 + 4;
const MAX_INLINE: usize = 3545;

#[derive(Debug, Error)]
pub enum EpochRowError {
    #[error(transparent)]
    Typed(#[from] TableError),
    #[error("invalid versioned row: {0}")]
    Invalid(String),
    #[error("unknown schema epoch {epoch} for table {table}")]
    UnknownEpoch { table: u64, epoch: u64 },
}
pub type EpochRowResult<T> = Result<T, EpochRowError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedEpochRow {
    pub table_id: TableId,
    pub epoch: u64,
    pub values: Vec<Value>,
}

/// Explicit registry: epoch IDs are validated schema-registration WAL positions,
/// never inferred from payload bytes. G36 supplies activation commit positions
/// for coordinated additive schema revisions.
#[derive(Default)]
pub struct EpochSchemas {
    tables: BTreeMap<(TableId, u64), Table>,
}
impl EpochSchemas {
    pub fn register(&mut self, epoch: u64, table: Table) -> EpochRowResult<()> {
        if epoch == 0 || table.id.0 == 0 {
            return Err(EpochRowError::Invalid("epoch and table IDs must be nonzero".into()));
        }
        crate::typed_table::validate_table(&table)?;
        match self.tables.get(&(table.id, epoch)) {
            Some(existing) if existing == &table => Ok(()),
            Some(_) => Err(EpochRowError::Invalid(
                "an epoch cannot be rebound to a different schema".into()
            )),
            None => {
                self.tables.insert((table.id, epoch), table);
                Ok(())
            }
        }
    }

    pub fn schema(&self, table_id: TableId, epoch: u64) -> EpochRowResult<&Table> {
        self.tables.get(&(table_id, epoch)).ok_or(
            EpochRowError::UnknownEpoch { table: table_id.0, epoch }
        )
    }

    pub fn encode(&self, table_id: TableId, epoch: u64, values: &[Value]) -> EpochRowResult<Vec<u8>> {
        let table = self.schema(table_id, epoch)?;
        let payload = encode_row(table, values)?;
        let size = u32::try_from(payload.len()).map_err(|_|
            EpochRowError::Invalid("payload size overflow".into())
        )?;
        if HEADER + payload.len() > MAX_INLINE {
            return Err(EpochRowError::Invalid("versioned row exceeds G22 inline limit".into()));
        }
        let mut out = Vec::with_capacity(HEADER + payload.len());
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&table_id.0.to_le_bytes());
        out.extend_from_slice(&epoch.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&hash(&payload).to_le_bytes());
        out.extend_from_slice(&payload);
        Ok(out)
    }

    /// Never fall back to a latest schema for an unknown epoch. All physical
    /// reads decode against the exactly registered immutable definition.
    pub fn decode(&self, bytes: &[u8]) -> EpochRowResult<DecodedEpochRow> {
        if bytes.len() < HEADER || &bytes[..4] != MAGIC || bytes[4] != VERSION {
            return Err(EpochRowError::Invalid("unknown envelope magic or version".into()));
        }
        let table_id = TableId(u64::from_le_bytes(bytes[5..13].try_into().unwrap()));
        let epoch = u64::from_le_bytes(bytes[13..21].try_into().unwrap());
        let declared = u32::from_le_bytes(bytes[21..25].try_into().unwrap()) as usize;
        let checksum = u32::from_le_bytes(bytes[25..29].try_into().unwrap());
        if epoch == 0 || table_id.0 == 0 || bytes.len() > MAX_INLINE ||
            declared != bytes.len() - HEADER
        {
            return Err(EpochRowError::Invalid("invalid row identities or payload length".into()));
        }
        let payload = &bytes[HEADER..];
        if hash(payload) != checksum {
            return Err(EpochRowError::Invalid("payload checksum mismatch".into()));
        }
        let schema = self.schema(table_id, epoch)?;
        let values = decode_row(schema, payload)?;
        Ok(DecodedEpochRow { table_id, epoch, values })
    }

    /// Decode pre-G32 BRRW bytes only with a caller-supplied *original*
    /// schema epoch. This does not guess and cannot reinterpret old layouts.
    pub fn decode_legacy(
        &self, table_id: TableId, epoch: u64, bytes: &[u8],
    ) -> EpochRowResult<DecodedEpochRow> {
        let schema = self.schema(table_id, epoch)?;
        let values = decode_row(schema, bytes)?;
        Ok(DecodedEpochRow { table_id, epoch, values })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, DataType, PrimaryKey, SchemaId};

    fn base() -> Table {
        Table {
            id: TableId(8), schema: SchemaId(2), name: "entries".into(),
            columns: vec![
                Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
                Column { name: "amount".into(), data_type: DataType::Int64, nullable: false },
            ],
            primary_key: PrimaryKey { columns: vec!["id".into()] },
            unique_keys: vec![], foreign_keys: vec![], column_defaults: Default::default(),
        }
    }

    #[test]
    fn roundtrip_two_epochs_without_reinterpreting_old_rows() {
        let original = base();
        let mut expanded = original.clone();
        expanded.columns.push(Column {
            name: "memo".into(), data_type: DataType::Text, nullable: true,
        });
        let mut schemas = EpochSchemas::default();
        schemas.register(7, original.clone()).unwrap();
        schemas.register(22, expanded).unwrap();
        let old = schemas.encode(original.id, 7, &[Value::Int64(1), Value::Int64(10)]).unwrap();
        let new = schemas.encode(original.id, 22, &[
            Value::Int64(1), Value::Int64(20), Value::Null,
        ]).unwrap();
        assert_eq!(schemas.decode(&old).unwrap().values,
            vec![Value::Int64(1), Value::Int64(10)]);
        assert_eq!(schemas.decode(&new).unwrap().values,
            vec![Value::Int64(1), Value::Int64(20), Value::Null]);
        let original_bytes = encode_row(&original, &[Value::Int64(2), Value::Int64(40)]).unwrap();
        assert_eq!(schemas.decode_legacy(original.id, 7, &original_bytes).unwrap().epoch, 7);
        assert!(schemas.decode(&original_bytes).is_err());
    }

    #[test]
    fn checksum_truncation_unknown_epoch_and_rebinding_fail_closed() {
        let original = base();
        let mut registry = EpochSchemas::default();
        registry.register(11, original.clone()).unwrap();
        let valid = registry.encode(original.id, 11, &[
            Value::Int64(1), Value::Int64(99),
        ]).unwrap();
        let mut corrupt = valid.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(registry.decode(&corrupt).is_err());
        assert!(registry.decode(&valid[..valid.len() - 1]).is_err());
        let mut unknown = valid.clone();
        unknown[13..21].copy_from_slice(&12_u64.to_le_bytes());
        assert!(matches!(registry.decode(&unknown), Err(EpochRowError::UnknownEpoch { .. })));
        let mut changed = original.clone();
        changed.columns[1].nullable = true;
        assert!(registry.register(11, changed).is_err());
    }

    #[test]
    fn invalid_source_values_and_capacity_are_rejected() {
        let original = base();
        let mut registry = EpochSchemas::default();
        registry.register(1, original.clone()).unwrap();
        assert!(registry.encode(original.id, 1, &[Value::Null, Value::Int64(4)]).is_err());
        assert!(registry.encode(original.id, 1, &[Value::Int64(1)]).is_err());
    }
}
