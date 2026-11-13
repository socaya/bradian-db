//! Persistent, schema-bound table rows built on the Bradian WAL-backed row store.
//!
//! This is a real table CRUD subsystem: it validates every value against the
//! catalog column definition, encodes rows and composite primary keys in a
//! versioned endian-stable format, enforces primary-key uniqueness for INSERT,
//! and uses optimistic LSN validation for atomic conditional writes.
//!
//! Secondary UNIQUE, FOREIGN KEY, CHECK, SQL execution and MVCC are not yet
//! implemented. A table declaring unsupported constraints is refused rather
//! than silently accepting rows that might violate them. The table schema is
//! pinned to its storage directory by a durable metadata record.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::{Column, DataType, Table};
use crate::runtime_limits::{ScanControl, ResourceError};
use crate::row_store::{RowMutation, RowStore, StoreError};

const ROW_MAGIC: &[u8; 4] = b"BRRW";
const KEY_MAGIC: &[u8; 4] = b"BRPK";
const VERSION: u8 = 1;
const META_KEY: &[u8] = b"\0bradian-table-schema-v1";
const MAX_RETRIES: usize = 16;

/// Portable values supported by the first typed row storage profile.
/// Decimal stores an exact signed unscaled integer; its scale is given by the
/// corresponding column type. TimestampTz is UTC microseconds since Unix epoch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Value {
    Null,
    Boolean(bool),
    Int32(i32),
    Int64(i64),
    Decimal(i128),
    String(String),
    TimestampTz(i64),
    Uuid([u8; 16]),
}

#[derive(Debug, Error)]
pub enum TableError {
    #[error(transparent)]
    Resource(#[from] ResourceError),
    #[error(transparent)]
    Storage(#[from] StoreError),
    #[error("invalid or unsupported table definition: {0}")]
    Schema(String),
    #[error("row has an invalid value: {0}")]
    Value(String),
    #[error("primary key already exists")]
    DuplicateKey,
    #[error("primary key does not exist")]
    NotFound,
    #[error("table changed concurrently; retry this operation")]
    Conflict,
    #[error("corrupt stored row: {0}")]
    Corruption(String),
}

pub type TableResult<T> = Result<T, TableError>;

pub(crate) fn decoded_row_bytes(row: &Vec<Value>) -> usize {
    std::mem::size_of::<Vec<Value>>().saturating_add(row.capacity().saturating_mul(std::mem::size_of::<Value>()))
        .saturating_add(row.iter().map(|value| match value { Value::String(text) => text.capacity(), _ => 0 }).fold(0_usize, usize::saturating_add))
}

/// Bound output amplification before cloning projected values, including duplicates.
pub(crate) fn project_rows_with_control(
    rows: Vec<Vec<Value>>, positions: &[usize], control: &ScanControl,
) -> Result<Vec<Vec<Value>>, ResourceError> {
    let mut meter = control.meter()?;
    let mut result = Vec::new();
    for row in rows {
        let mut bytes = std::mem::size_of::<Vec<Value>>().saturating_add(positions.len().saturating_mul(std::mem::size_of::<Value>()));
        for position in positions {
            meter.step()?;
            if let Value::String(text) = &row[*position] { bytes = bytes.saturating_add(text.len()); }
        }
        meter.row(bytes)?;
        result.push(positions.iter().map(|position|row[*position].clone()).collect());
    }
    control.check()?;
    Ok(result)
}

/// One persistent table mapped to an independently locked RowStore directory.
/// Table IDs must remain stable across process restarts; the pinned schema
/// document prevents a different table from reusing this directory.
pub struct TableStore {
    table: Table,
    storage: RowStore,
    pk_positions: Vec<usize>,
}

impl TableStore {
    pub fn open(path: impl AsRef<Path>, table: Table) -> TableResult<Self> {
        let pk_positions = validate_table(&table)?;
        let storage = RowStore::open(path)?;
        let expected = serde_json::to_vec(&table)
            .map_err(|e| TableError::Schema(format!("cannot serialize table definition: {e}")))?;
        let stored = storage.get(META_KEY);
        match stored {
            Some(current) if current != expected =>
                return Err(TableError::Schema("stored table definition differs from supplied schema".into())),
            Some(_) => {}
            None => {
                if !storage.is_empty() {
                    return Err(TableError::Schema("existing store has rows but no table metadata".into()));
                }
                storage.put(META_KEY, &expected)?;
            }
        }
        Ok(Self { table, storage, pk_positions })
    }

    /// Start a single-table optimistic transaction with a consistent row
    /// snapshot. No locks are held across application work, and rollback
    /// never writes to disk. Commit uses one synchronized BRWL batch.
    pub fn begin_transaction(&self) -> TableResult<TableTransaction<'_>> {
        self.begin_transaction_with_control(&ScanControl::legacy_unlimited())
    }

    pub fn begin_transaction_with_control(&self, control: &ScanControl) -> TableResult<TableTransaction<'_>> {
        let mut meter = control.meter()?;
        let (base_lsn, records) = self.storage.snapshot_with_control(Some(META_KEY), control)?;
        let mut snapshot = BTreeMap::new();
        for (key, bytes) in records {
            if key.as_slice() == META_KEY { continue; }
            if !key.starts_with(KEY_MAGIC) {
                return Err(TableError::Corruption("unexpected key namespace in table storage".into()));
            }
            let row = decode_row(&self.table, &bytes)?;
            if self.key_from_row(&row)? != key {
                return Err(TableError::Corruption("stored key does not match row primary key".into()));
            }
            meter.row(decoded_row_bytes(&row).saturating_add(key.capacity()).saturating_add(std::mem::size_of::<Vec<u8>>()))?;
            snapshot.insert(key, row);
        }
        control.check()?;
        Ok(TableTransaction {
            table: self,
            base_lsn,
            snapshot,
            changes: BTreeMap::new(),
        })
    }

    pub fn table(&self) -> &Table { &self.table }
    pub fn last_lsn(&self) -> u64 { self.storage.last_lsn() }
    pub fn checkpoint(&self) -> TableResult<u64> { Ok(self.storage.checkpoint()?) }

    /// INSERT is non-upserting and atomically checks primary-key uniqueness.
    pub fn insert(&self, row: &[Value]) -> TableResult<u64> {
        let encoded = encode_row(&self.table, row)?;
        let key = self.key_from_row(row)?;
        for _ in 0..MAX_RETRIES {
            let version = self.storage.last_lsn();
            if self.storage.get(&key).is_some() {
                return Err(TableError::DuplicateKey);
            }
            match self.storage.commit_batch(Some(version), &[
                RowMutation::Put { key: key.clone(), value: encoded.clone() },
            ]) {
                Ok(lsn) => return Ok(lsn),
                Err(StoreError::Conflict { .. }) => continue,
                Err(other) => return Err(other.into()),
            }
        }
        Err(TableError::Conflict)
    }

    /// UPDATE replaces an existing row, but may not alter its primary key.
    pub fn replace(&self, row: &[Value]) -> TableResult<u64> {
        let encoded = encode_row(&self.table, row)?;
        let key = self.key_from_row(row)?;
        for _ in 0..MAX_RETRIES {
            let version = self.storage.last_lsn();
            if self.storage.get(&key).is_none() {
                return Err(TableError::NotFound);
            }
            match self.storage.commit_batch(Some(version), &[
                RowMutation::Put { key: key.clone(), value: encoded.clone() },
            ]) {
                Ok(lsn) => return Ok(lsn),
                Err(StoreError::Conflict { .. }) => continue,
                Err(other) => return Err(other.into()),
            }
        }
        Err(TableError::Conflict)
    }

    /// DELETE returns NotFound when no row matches the supplied primary key.
    pub fn delete(&self, primary_key: &[Value]) -> TableResult<u64> {
        let key = encode_key(&self.table, &self.pk_positions, primary_key)?;
        for _ in 0..MAX_RETRIES {
            let version = self.storage.last_lsn();
            if self.storage.get(&key).is_none() {
                return Err(TableError::NotFound);
            }
            match self.storage.commit_batch(Some(version), &[
                RowMutation::Delete { key: key.clone() },
            ]) {
                Ok(lsn) => return Ok(lsn),
                Err(StoreError::Conflict { .. }) => continue,
                Err(other) => return Err(other.into()),
            }
        }
        Err(TableError::Conflict)
    }

    pub fn get(&self, primary_key: &[Value]) -> TableResult<Option<Vec<Value>>> {
        let key = encode_key(&self.table, &self.pk_positions, primary_key)?;
        self.storage.indexed_get(&key)?.map(|bytes| {
            let row = decode_row(&self.table, &bytes)?;
            if self.key_from_row(&row)? != key {
                return Err(TableError::Corruption("stored primary key does not match row data".into()));
            }
            Ok(row)
        }).transpose()
    }

    pub fn scan(&self) -> TableResult<Vec<Vec<Value>>> {
        self.scan_with_control(&ScanControl::legacy_unlimited())
    }

    pub fn scan_with_control(&self, control: &ScanControl) -> TableResult<Vec<Vec<Value>>> {
        let mut meter = control.meter()?;
        let (_, records) = self.storage.snapshot_with_control(Some(META_KEY), control)?;
        let mut result = Vec::new();
        for (key, bytes) in records {
            if key.as_slice() == META_KEY { continue; }
            if !key.starts_with(KEY_MAGIC) {
                return Err(TableError::Corruption("unexpected key namespace in table storage".into()));
            }
            let row = decode_row(&self.table, &bytes)?;
            if key != self.key_from_row(&row)? {
                return Err(TableError::Corruption("stored primary key does not match row data".into()));
            }
            meter.row(decoded_row_bytes(&row))?;
            result.push(row);
        }
        control.check()?;
        Ok(result)
    }

    fn key_from_row(&self, row: &[Value]) -> TableResult<Vec<u8>> {
        if row.len() != self.table.columns.len() {
            return Err(TableError::Value("row width does not match table schema".into()));
        }
        let pk = self.pk_positions.iter().map(|i| row[*i].clone()).collect::<Vec<_>>();
        encode_key(&self.table, &self.pk_positions, &pk)
    }
}

pub(crate) fn validate_table(table: &Table) -> TableResult<Vec<usize>> {
    if !table.unique_keys.is_empty() || !table.foreign_keys.is_empty() {
        return Err(TableError::Schema(
            "secondary UNIQUE and FOREIGN KEY constraints are not implemented in this table store".into()
        ));
    }
    if table.id.0 == 0 || table.columns.is_empty()
        || table.columns.len() > u16::MAX as usize
        || table.primary_key.columns.is_empty()
    {
        return Err(TableError::Schema("table requires stable nonzero identity, supported column count and primary key".into()));
    }
    let mut names = BTreeSet::new();
    for column in &table.columns {
        if !names.insert(column.name.as_str()) {
            return Err(TableError::Schema("duplicate column name".into()));
        }
    }
    for (name, value) in &table.column_defaults {
        let column = table.columns.iter().find(|column| &column.name == name)
            .ok_or_else(|| TableError::Schema(format!("default references unknown column {name}")))?;
        validate_value(column, value).map_err(|err| TableError::Schema(format!("invalid default: {err}")))?;
    }
    let mut positions = Vec::new();
    let mut selected = BTreeSet::new();
    for pk in &table.primary_key.columns {
        let pos = table.columns.iter().position(|c| &c.name == pk)
            .ok_or_else(|| TableError::Schema(format!("missing primary-key column {pk}")))?;
        if !selected.insert(pos) || table.columns[pos].nullable {
            return Err(TableError::Schema("primary key must have distinct NOT NULL columns".into()));
        }
        positions.push(pos);
    }
    Ok(positions)
}

fn validate_value(column: &Column, value: &Value) -> TableResult<()> {
    if matches!(value, Value::Null) {
        return if column.nullable { Ok(()) } else {
            Err(TableError::Value(format!("{} is NOT NULL", column.name)))
        };
    }
    let valid = match (&column.data_type, value) {
        (DataType::Boolean, Value::Boolean(_)) |
        (DataType::Int32, Value::Int32(_)) |
        (DataType::Int64, Value::Int64(_)) |
        (DataType::Text, Value::String(_)) |
        (DataType::TimestampTz, Value::TimestampTz(_)) |
        (DataType::Uuid, Value::Uuid(_)) => true,
        (DataType::Varchar { max_length }, Value::String(s)) =>
            s.len() <= *max_length as usize,
        (DataType::Decimal { precision, scale }, Value::Decimal(v)) => {
            if *precision == 0 || *precision > 38 || *scale > *precision { false }
            else {
                let magnitude = v.unsigned_abs();
                let limit = 10_u128.pow(*precision as u32);
                magnitude < limit
            }
        }
        _ => false,
    };
    if valid { Ok(()) } else {
        Err(TableError::Value(format!("{} does not match its declared type or bounds", column.name)))
    }
}

fn write_value(out: &mut Vec<u8>, value: &Value) -> TableResult<()> {
    match value {
        Value::Null => out.push(0),
        Value::Boolean(v) => { out.push(1); out.push(u8::from(*v)); }
        Value::Int32(v) => { out.push(2); out.extend_from_slice(&v.to_le_bytes()); }
        Value::Int64(v) => { out.push(3); out.extend_from_slice(&v.to_le_bytes()); }
        Value::Decimal(v) => { out.push(4); out.extend_from_slice(&v.to_le_bytes()); }
        Value::String(s) => {
            out.push(5);
            let len = u32::try_from(s.len())
                .map_err(|_| TableError::Value("text length exceeds u32".into()))?;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(s.as_bytes());
        }
        Value::TimestampTz(v) => { out.push(6); out.extend_from_slice(&v.to_le_bytes()); }
        Value::Uuid(v) => { out.push(7); out.extend_from_slice(v); }
    }
    Ok(())
}

fn read_value(input: &[u8], pos: &mut usize) -> TableResult<Value> {
    let tag = *input.get(*pos).ok_or_else(|| TableError::Corruption("missing type tag".into()))?;
    *pos += 1;
    let width = match tag {
        0 => return Ok(Value::Null),
        1 => 1,
        2 => 4,
        3 | 6 => 8,
        4 | 7 => 16,
        5 => 4,
        _ => return Err(TableError::Corruption("unrecognized row value type tag".into())),
    };
    if input.len().saturating_sub(*pos) < width {
        return Err(TableError::Corruption("truncated row value".into()));
    }
    let data = &input[*pos..*pos + width];
    *pos += width;
    Ok(match tag {
        1 => match data[0] {
            0 => Value::Boolean(false), 1 => Value::Boolean(true),
            _ => return Err(TableError::Corruption("invalid Boolean representation".into())),
        },
        2 => Value::Int32(i32::from_le_bytes(data.try_into().unwrap())),
        3 => Value::Int64(i64::from_le_bytes(data.try_into().unwrap())),
        4 => Value::Decimal(i128::from_le_bytes(data.try_into().unwrap())),
        5 => {
            let length = u32::from_le_bytes(data.try_into().unwrap()) as usize;
            if input.len().saturating_sub(*pos) < length {
                return Err(TableError::Corruption("truncated text data".into()));
            }
            let text = String::from_utf8(input[*pos..*pos + length].to_vec())
                .map_err(|_| TableError::Corruption("invalid UTF-8 text".into()))?;
            *pos += length;
            Value::String(text)
        }
        6 => Value::TimestampTz(i64::from_le_bytes(data.try_into().unwrap())),
        7 => Value::Uuid(data.try_into().unwrap()),
        _ => unreachable!(),
    })
}

pub fn encode_row(table: &Table, values: &[Value]) -> TableResult<Vec<u8>> {
    if values.len() != table.columns.len() {
        return Err(TableError::Value("row width does not match table schema".into()));
    }
    let count = u16::try_from(values.len())
        .map_err(|_| TableError::Value("too many columns".into()))?;
    let mut out = Vec::new();
    out.extend_from_slice(ROW_MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&table.id.0.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    for (column, value) in table.columns.iter().zip(values) {
        validate_value(column, value)?;
        write_value(&mut out, value)?;
    }
    Ok(out)
}

pub fn decode_row(table: &Table, bytes: &[u8]) -> TableResult<Vec<Value>> {
    if bytes.len() < 15 || &bytes[..4] != ROW_MAGIC || bytes[4] != VERSION {
        return Err(TableError::Corruption("invalid row format or version".into()));
    }
    let table_id = u64::from_le_bytes(bytes[5..13].try_into().unwrap());
    let count = u16::from_le_bytes(bytes[13..15].try_into().unwrap()) as usize;
    if table_id != table.id.0 || count != table.columns.len() {
        return Err(TableError::Corruption("row table identity or column count mismatch".into()));
    }
    let mut pos = 15;
    let mut row = Vec::with_capacity(count);
    for column in &table.columns {
        let value = read_value(bytes, &mut pos)?;
        validate_value(column, &value)
            .map_err(|_| TableError::Corruption("stored value violates table schema".into()))?;
        row.push(value);
    }
    if pos != bytes.len() {
        return Err(TableError::Corruption("unexpected trailing row bytes".into()));
    }
    Ok(row)
}

pub(crate) fn encode_key(table: &Table, pk_positions: &[usize], pk: &[Value]) -> TableResult<Vec<u8>> {
    if pk.len() != pk_positions.len() {
        return Err(TableError::Value("primary-key width mismatch".into()));
    }
    let mut out = Vec::new();
    out.extend_from_slice(KEY_MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&table.id.0.to_le_bytes());
    for (index, value) in pk_positions.iter().zip(pk) {
        let column = &table.columns[*index];
        validate_value(column, value)?;
        if matches!(value, Value::Null) {
            return Err(TableError::Value("NULL primary key".into()));
        }
        write_value(&mut out, value)?;
    }
    if out.len() > 512 {
        return Err(TableError::Value("encoded primary key exceeds 512-byte storage limit".into()));
    }
    Ok(out)
}


/// One optimistic, snapshot-based, single-table transaction.
///
/// Every mutation is staged in memory; a successful commit serializes all
/// changed keys into exactly one CRC-protected, fsynced BRWL batch frame.
/// The global LSN compare-and-swap rejects concurrent writes to the same
/// RowStore, including unrelated keys. This conservative validation rules
/// out write skew within a single RowStore, but is not multi-table MVCC.
pub struct TableTransaction<'a> {
    table: &'a TableStore,
    base_lsn: u64,
    snapshot: BTreeMap<Vec<u8>, Vec<Value>>,
    changes: BTreeMap<Vec<u8>, Option<Vec<Value>>>,
}

impl TableTransaction<'_> {
    pub fn start_lsn(&self) -> u64 { self.base_lsn }

    /// Return a row as observed by this transaction, including its own
    /// staged writes, independent of later external commits.
    pub fn get(&self, primary_key: &[Value]) -> TableResult<Option<Vec<Value>>> {
        let key = encode_key(&self.table.table, &self.table.pk_positions, primary_key)?;
        Ok(match self.changes.get(&key) {
            Some(value) => value.clone(),
            None => self.snapshot.get(&key).cloned(),
        })
    }

    /// Return ordered rows as of the initial snapshot plus staged mutations.
    pub fn scan(&self) -> Vec<Vec<Value>> {
        let mut rows = self.snapshot.clone();
        for (key, value) in &self.changes {
            match value {
                Some(value) => { rows.insert(key.clone(), value.clone()); }
                None => { rows.remove(key); }
            }
        }
        rows.into_values().collect()
    }

    /// Merge borrowed maps; do not clone the complete transaction snapshot.
    pub fn scan_with_control(&self, control: &ScanControl) -> TableResult<Vec<Vec<Value>>> {
        let mut meter = control.meter()?;
        let mut snapshot = self.snapshot.iter().peekable();
        let mut changes = self.changes.iter().peekable();
        let mut rows = Vec::new();
        while snapshot.peek().is_some() || changes.peek().is_some() {
            meter.step()?;
            let use_change = match (snapshot.peek(), changes.peek()) {
                (Some((a, _)), Some((b, _))) => b <= a,
                (None, Some(_)) => true,
                _ => false,
            };
            let row = if use_change {
                let (key, value) = changes.next().expect("staged key exists");
                if snapshot.peek().is_some_and(|(other, _)| *other == key) { snapshot.next(); }
                value.as_ref()
            } else { Some(snapshot.next().expect("snapshot key exists").1) };
            if let Some(row) = row {
                meter.row(decoded_row_bytes(row))?;
                rows.push(row.clone());
            }
        }
        control.check()?;
        Ok(rows)
    }

    pub fn insert(&mut self, row: &[Value]) -> TableResult<()> {
        let bytes = encode_row(&self.table.table, row)?;
        let key = self.table.key_from_row(row)?;
        if bytes.len() > 3546 {
            return Err(TableError::Value("encoded row exceeds inline storage capacity".into()));
        }
        if self.visible(&key).is_some() {
            return Err(TableError::DuplicateKey);
        }
        self.changes.insert(key, Some(row.to_vec()));
        Ok(())
    }

    pub fn replace(&mut self, row: &[Value]) -> TableResult<()> {
        let bytes = encode_row(&self.table.table, row)?;
        let key = self.table.key_from_row(row)?;
        if bytes.len() > 3546 {
            return Err(TableError::Value("encoded row exceeds inline storage capacity".into()));
        }
        if self.visible(&key).is_none() {
            return Err(TableError::NotFound);
        }
        self.changes.insert(key, Some(row.to_vec()));
        Ok(())
    }

    pub fn delete(&mut self, primary_key: &[Value]) -> TableResult<()> {
        let key = encode_key(&self.table.table, &self.table.pk_positions, primary_key)?;
        if self.visible(&key).is_none() {
            return Err(TableError::NotFound);
        }
        self.changes.insert(key, None);
        Ok(())
    }

    fn visible(&self, key: &[u8]) -> Option<&Vec<Value>> {
        match self.changes.get(key) {
            Some(Some(value)) => Some(value),
            Some(None) => None,
            None => self.snapshot.get(key),
        }
    }

    /// Abort without appending any WAL records. Dropping an uncommitted
    /// transaction has the same effect.
    pub fn rollback(self) {}

    /// Commit all staged final row states in one durable WAL frame.
    ///
    /// On stale LSN, the entire operation is rejected, including read-only
    /// dependency conflicts; no partial rows are published.
    pub fn commit(self) -> TableResult<u64> {
        if self.changes.is_empty() {
            // A read-only transaction has a stable owned snapshot and does
            // not need a WAL record or write-conflict validation.
            return Ok(self.base_lsn);
        }
        let mut operations = Vec::with_capacity(self.changes.len());
        for (key, value) in self.changes {
            match value {
                Some(row) => {
                    let bytes = encode_row(&self.table.table, &row)?;
                    operations.push(RowMutation::Put { key, value: bytes });
                }
                None => operations.push(RowMutation::Delete { key }),
            }
        }
        match self.table.storage.commit_batch(Some(self.base_lsn), &operations) {
            Ok(lsn) => Ok(lsn),
            Err(StoreError::Conflict { .. }) => Err(TableError::Conflict),
            Err(err) => Err(TableError::Storage(err)),
        }
    }
}
