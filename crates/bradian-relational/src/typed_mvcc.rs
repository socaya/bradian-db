//! G21 typed-table adapter for G20's opt-in MVCC engine.
//!
//! Schema pins and row bytes use exactly the G15 shared-domain format. This
//! module opens a *previously registered* domain and validates every schema
//! and physical row against the immutable G11 catalog before permitting SQL.
//! No schema registration, auto-migration, or metadata writes happen here.
//! The G20 store's exclusive RowStore lock prevents G15/G16 writers from
//! concurrently changing the domain without its version history knowing.

use std::collections::BTreeMap;
use std::path::Path;

use crate::{Table, TableId};
use crate::mvcc::{MvccError, MvccStats, MvccStore, MvccTransaction};
use crate::typed_table::{
    decode_row, encode_key, encode_row, validate_table, TableError, Value,
};
use crate::transaction_domain::DomainError;

const SCHEMA_PREFIX: &[u8] = b"\0bradian-domain-table-v1:";
const ROW_PREFIX: &[u8] = b"BRPK";
const INLINE_CAPACITY: usize = 3546;

#[derive(Clone)]
struct Definition {
    table: Table,
    pk_positions: Vec<usize>,
}

fn table_id_from_row_key(key: &[u8]) -> Result<TableId, DomainError> {
    if key.len() < 13 || &key[..4] != ROW_PREFIX || key[4] != 1 {
        return Err(DomainError::Corruption(
            "MVCC domain contains unrecognized row namespace/version".into(),
        ));
    }
    Ok(TableId(u64::from_le_bytes(key[5..13].try_into().unwrap())))
}

fn key_from_row(def: &Definition, row: &[Value]) -> Result<Vec<u8>, DomainError> {
    if row.len() != def.table.columns.len() {
        return Err(DomainError::Table(TableError::Value(
            "row width does not match pinned schema".into(),
        )));
    }
    let pk = def.pk_positions.iter().map(|index| row[*index].clone()).collect::<Vec<_>>();
    Ok(encode_key(&def.table, &def.pk_positions, &pk)?)
}

/// A validated, exclusively owned view of a G15-formatted shared WAL.
pub struct TypedMvccStore {
    store: MvccStore,
    tables: BTreeMap<TableId, Definition>,
}

impl TypedMvccStore {
    /// Requires a G15/G18-registered domain; does not create metadata pins.
    /// All catalog tables need not be active, but all *stored* domain pins
    /// must match precisely the supplied authoritative catalog definitions.
    pub fn open(
        domain_root: impl AsRef<Path>,
        catalog_tables: &BTreeMap<TableId, Table>,
    ) -> Result<Self, DomainError> {
        let store = MvccStore::open(domain_root).map_err(map_mvcc)?;
        let mut snapshot = store.begin();
        let records = snapshot.scan();
        snapshot.rollback();
        let mut tables = BTreeMap::new();
        for (key, bytes) in &records {
            if !key.starts_with(SCHEMA_PREFIX) { continue; }
            if key.len() != SCHEMA_PREFIX.len() + 8 {
                return Err(DomainError::Corruption("malformed shared-domain schema key".into()));
            }
            let id = TableId(u64::from_be_bytes(
                key[SCHEMA_PREFIX.len()..].try_into().unwrap()
            ));
            let table: Table = serde_json::from_slice(bytes)
                .map_err(|err| DomainError::Corruption(format!(
                    "malformed schema pin {}: {err}", id.0
                )))?;
            if table.id != id || catalog_tables.get(&id) != Some(&table) {
                return Err(DomainError::Corruption(format!(
                    "pinned schema {} differs from durable catalog", id.0
                )));
            }
            let pk_positions = validate_table(&table)?;
            if tables.insert(id, Definition { table, pk_positions }).is_some() {
                return Err(DomainError::Corruption("duplicate schema pin".into()));
            }
        }
        if tables.is_empty() {
            return Err(DomainError::Corruption(
                "shared MVCC domain has no registered catalog table schemas".into(),
            ));
        }
        for (key, bytes) in &records {
            if key.starts_with(SCHEMA_PREFIX) { continue; }
            let table_id = table_id_from_row_key(key)?;
            let schema = tables.get(&table_id).ok_or_else(|| {
                DomainError::Corruption(format!(
                    "physical row refers to unregistered table {}", table_id.0
                ))
            })?;
            let row = decode_row(&schema.table, bytes)?;
            if key_from_row(schema, &row)? != *key {
                return Err(DomainError::Corruption(
                    "stored row key differs from decoded primary key".into(),
                ));
            }
        }
        Ok(Self { store, tables })
    }

    pub fn active_tables(&self) -> Vec<TableId> {
        self.tables.keys().copied().collect()
    }

    pub fn table(&self, id: TableId) -> Option<&Table> {
        self.tables.get(&id).map(|definition| &definition.table)
    }

    pub fn stats(&self) -> MvccStats { self.store.stats() }

    pub fn last_lsn(&self) -> u64 { self.store.committed_lsn() }

    pub fn checkpoint(&self) -> Result<u64, DomainError> {
        self.store.checkpoint().map_err(map_mvcc)
    }

    pub fn begin(&self) -> TypedMvccTransaction<'_> {
        TypedMvccTransaction { owner: self, inner: self.store.begin() }
    }
}

fn map_mvcc(err: MvccError) -> DomainError {
    match err {
        MvccError::Storage(err) => DomainError::Storage(err),
        MvccError::DuplicateKey => DomainError::Table(TableError::DuplicateKey),
        MvccError::NotFound => DomainError::Table(TableError::NotFound),
        MvccError::WriteConflict | MvccError::ScanConflict => DomainError::Conflict,
        other => DomainError::Corruption(other.to_string()),
    }
}

/// Typed SQL-accessible transaction over G20 versions and G15 physical keys.
pub struct TypedMvccTransaction<'a> {
    owner: &'a TypedMvccStore,
    inner: MvccTransaction<'a>,
}

impl TypedMvccTransaction<'_> {
    pub fn start_lsn(&self) -> u64 { self.inner.start_lsn() }

    fn definition(&self, id: TableId) -> Result<&Definition, DomainError> {
        self.owner.tables.get(&id).ok_or(DomainError::UnknownTable(id.0))
    }

    pub fn get(&self, id: TableId, key: &[Value]) -> Result<Option<Vec<Value>>, DomainError> {
        let def = self.definition(id)?;
        let physical_key = encode_key(&def.table, &def.pk_positions, key)?;
        self.inner.get(&physical_key).map(|bytes| {
            let row = decode_row(&def.table, &bytes)?;
            if key_from_row(def, &row)? != physical_key {
                return Err(DomainError::Corruption(
                    "MVCC row does not match its physical primary key".into(),
                ));
            }
            Ok(row)
        }).transpose()
    }

    pub fn scan(&mut self, id: TableId) -> Result<Vec<Vec<Value>>, DomainError> {
        if !self.owner.tables.contains_key(&id) {
            return Err(DomainError::UnknownTable(id.0));
        }
        let records = self.inner.scan();
        let mut rows = Vec::new();
        for (key, bytes) in records {
            // The full physical scan participates in G20's conservative
            // global LSN change validation when the transaction writes.
            if key.starts_with(SCHEMA_PREFIX) { continue; }
            let table_id = table_id_from_row_key(&key)?;
            if table_id != id { continue; }
            let def = self.definition(id)?;
            let row = decode_row(&def.table, &bytes)?;
            if key_from_row(def, &row)? != key {
                return Err(DomainError::Corruption(
                    "MVCC scan found row with mismatched physical key".into(),
                ));
            }
            rows.push(row);
        }
        Ok(rows)
    }

    pub fn insert(&mut self, id: TableId, row: &[Value]) -> Result<(), DomainError> {
        let def = self.definition(id)?;
        let bytes = encode_row(&def.table, row)?;
        if bytes.len() > INLINE_CAPACITY {
            return Err(DomainError::Table(TableError::Value(
                "encoded row exceeds physical inline limit".into()
            )));
        }
        let key = key_from_row(def, row)?;
        self.inner.insert(&key, &bytes).map_err(map_mvcc)
    }

    pub fn replace(&mut self, id: TableId, row: &[Value]) -> Result<(), DomainError> {
        let def = self.definition(id)?;
        let bytes = encode_row(&def.table, row)?;
        if bytes.len() > INLINE_CAPACITY {
            return Err(DomainError::Table(TableError::Value(
                "encoded row exceeds physical inline limit".into()
            )));
        }
        let key = key_from_row(def, row)?;
        self.inner.replace(&key, &bytes).map_err(map_mvcc)
    }

    pub fn delete(&mut self, id: TableId, pk: &[Value]) -> Result<(), DomainError> {
        let def = self.definition(id)?;
        let key = encode_key(&def.table, &def.pk_positions, pk)?;
        self.inner.delete(&key).map_err(map_mvcc)
    }

    pub fn rollback(self) { self.inner.rollback(); }

    pub fn commit(self) -> Result<u64, DomainError> {
        self.inner.commit().map_err(map_mvcc)
    }
}
