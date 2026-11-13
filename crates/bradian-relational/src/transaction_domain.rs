//! Native multi-table atomic transaction domain for Bradian (G15).
//!
//! Unlike independent G4 TableStore directories, all registered tables in
//! this domain use ONE RowStore, ONE WAL and ONE checkpoint generation. A
//! multi-table commit is therefore exactly one CRC-checked, synchronized
//! BRWL batch, with no two-phase-commit ambiguity or partially committed
//! data files. This implementation does NOT atomically update the separately
//! persisted G11 catalog, nor does it migrate existing TableStore files.
//!
//! Every table schema is durably pinned in this domain. Table identities
//! are supplied by a previously validated catalog and cannot be reused
//! for a different schema. Secondary UNIQUE/FK enforcement is deliberately
//! refused until a transaction-wide constraint implementation exists.

use std::collections::BTreeMap;
use std::path::Path;

use parking_lot::Mutex;
use thiserror::Error;

use crate::{Table, TableId};
use crate::backup::BackupReceipt;
use crate::row_store::{RowMutation, RowStore, StoreError};
use crate::typed_table::{
    decode_row, encode_key, encode_row, validate_table, TableError, Value,
};

const META_PREFIX: &[u8] = b"\0bradian-domain-table-v1:";
const KEY_MAGIC: &[u8] = b"BRPK";
const MAX_INLINE_VALUE: usize = 3546;
const MAX_SCHEMA_BYTES: usize = MAX_INLINE_VALUE;
const MAX_RETRIES: usize = 16;

#[derive(Debug, Error)]
pub enum DomainError {
    #[error(transparent)]
    Resource(#[from] crate::runtime_limits::ResourceError),
    #[error(transparent)]
    Storage(#[from] StoreError),
    #[error(transparent)]
    Table(#[from] TableError),
    #[error(transparent)]
    Encoding(#[from] serde_json::Error),
    #[error("transaction domain corruption: {0}")]
    Corruption(String),
    #[error("table {0} is not registered in this transaction domain")]
    UnknownTable(u64),
    #[error("registered table {0} has an incompatible schema")]
    SchemaMismatch(u64),
    #[error("invalid savepoint operation: {0}")]
    Savepoint(String),
    #[error("transaction conflict: concurrent write changed domain state")]
    Conflict,
}

pub type DomainResult<T> = Result<T, DomainError>;

#[derive(Debug, Clone)]
struct Registered {
    table: Table,
    pk_positions: Vec<usize>,
}

/// All tables written through this API share one persistent WAL and one
/// physical checkpoint. The exclusive RowStore lock owns the directory.
#[derive(Debug)]
pub struct MultiTableStore {
    storage: RowStore,
    tables: Mutex<BTreeMap<TableId, Registered>>,
}

impl MultiTableStore {
    /// Reopen the authoritative store and verify every registered table's
    /// schema and every persisted row against the pinned table identity.
    pub fn open(root: impl AsRef<Path>) -> DomainResult<Self> {
        let storage = RowStore::open(root)?;
        let (_, entries) = storage.snapshot_with_lsn();
        let mut tables = BTreeMap::new();
        for (key, bytes) in &entries {
            if !key.starts_with(META_PREFIX) { continue; }
            if key.len() != META_PREFIX.len() + 8 {
                return Err(DomainError::Corruption("invalid table metadata key length".into()));
            }
            let id = u64::from_be_bytes(key[META_PREFIX.len()..].try_into().unwrap());
            let table: Table = serde_json::from_slice(bytes)
                .map_err(|e| DomainError::Corruption(format!("table {id} metadata: {e}")))?;
            if table.id.0 != id {
                return Err(DomainError::Corruption("metadata table identity mismatch".into()));
            }
            let positions = validate_table(&table)?;
            if tables.insert(table.id, Registered { table, pk_positions: positions }).is_some() {
                return Err(DomainError::Corruption("duplicate metadata table identity".into()));
            }
        }
        // No orphan record is accepted: all data must decode through its
        // exact pinned table and reproduce its physical primary-key bytes.
        for (key, bytes) in &entries {
            if key.starts_with(META_PREFIX) { continue; }
            let table_id = row_table_id(key)?;
            let schema = tables.get(&table_id)
                .ok_or_else(|| DomainError::Corruption(format!("row for unregistered table {}", table_id.0)))?;
            let row = decode_row(&schema.table, bytes)?;
            if make_key(schema, &row)? != *key {
                return Err(DomainError::Corruption("physical primary key differs from decoded row".into()));
            }
        }
        Ok(Self { storage, tables: Mutex::new(tables) })
    }

    /// Pin a catalog-validated schema durably. This operation is idempotent
    /// for identical schemas and rejects reuse of a TableId with new metadata.
    /// Registration takes one WAL LSN but is not a separate catalog DDL
    /// transaction; callers must coordinate their external catalog lifecycle.
    pub fn register_table(&self, table: Table) -> DomainResult<u64> {
        let positions = validate_table(&table)?;
        let key = metadata_key(table.id);
        let bytes = serde_json::to_vec(&table)?;
        if bytes.len() > MAX_SCHEMA_BYTES {
            return Err(DomainError::Table(TableError::Schema(
                "table metadata exceeds inline storage size".into(),
            )));
        }
        let mut registered = self.tables.lock();
        if let Some(existing) = registered.get(&table.id) {
            if existing.table != table { return Err(DomainError::SchemaMismatch(table.id.0)); }
            return Ok(self.storage.last_lsn());
        }
        for _ in 0..MAX_RETRIES {
            let lsn = self.storage.last_lsn();
            // Physical metadata already present is always checked; we never
            // replace a persisted schema on the basis of an in-memory cache.
            if let Some(existing) = self.storage.get(&key) {
                if existing != bytes { return Err(DomainError::SchemaMismatch(table.id.0)); }
                registered.insert(table.id, Registered { table, pk_positions: positions });
                return Ok(lsn);
            }
            match self.storage.commit_batch(Some(lsn), &[RowMutation::Put {
                key: key.clone(), value: bytes.clone(),
            }]) {
                Ok(committed) => {
                    registered.insert(table.id, Registered { table, pk_positions: positions });
                    return Ok(committed);
                }
                Err(StoreError::Conflict { .. }) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(DomainError::Conflict)
    }

    /// Owned schema pins used by lifecycle integrity reconciliation. Never
    /// expose the internal registry mutex to callers.
    pub fn registered_tables(&self) -> Vec<Table> {
        self.tables.lock().values().map(|entry| entry.table.clone()).collect()
    }

    pub fn table_count(&self) -> usize { self.tables.lock().len() }
    pub fn last_lsn(&self) -> u64 { self.storage.last_lsn() }
    pub(crate) fn storage_row_count(&self) -> u64 { self.storage.len() as u64 }
    pub fn checkpoint(&self) -> DomainResult<u64> { Ok(self.storage.checkpoint()?) }

    /// Back up the entire registered multi-table domain as one verified WAL
    /// generation. All schema pins and row data are stored under this same
    /// RowStore. External G11 catalog directories are NOT included.
    pub fn backup_to(&self, destination: impl AsRef<Path>) -> DomainResult<BackupReceipt> {
        Ok(self.storage.backup_to(destination)?)
    }

    /// Restore a previously verified shared-domain backup into an absent
    /// directory. Reopen the domain afterwards to verify schema/data bindings.
    pub fn restore_backup(
        backup: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> DomainResult<BackupReceipt> {
        Ok(RowStore::restore_backup(backup, destination)?)
    }

    /// Capture a consistent domain snapshot. A registration concurrent with
    /// this call is serialized by the registry mutex; row contents and LSN
    /// are captured together by RowStore.
    pub fn begin_transaction(&self) -> DomainResult<MultiTableTransaction<'_>> {
        let definitions = self.tables.lock().clone();
        let (base_lsn, records) = self.storage.snapshot_with_lsn();
        let mut snapshot = BTreeMap::new();
        for (key, bytes) in records {
            if key.starts_with(META_PREFIX) { continue; }
            let table_id = row_table_id(&key)?;
            let definition = definitions.get(&table_id)
                .ok_or_else(|| DomainError::Corruption("row belongs to unregistered table".into()))?;
            let decoded = decode_row(&definition.table, &bytes)?;
            if make_key(definition, &decoded)? != key {
                return Err(DomainError::Corruption("row key does not match encoded value".into()));
            }
            snapshot.insert(key, decoded);
        }
        Ok(MultiTableTransaction {
            domain: self,
            definitions,
            base_lsn,
            snapshot,
            changes: BTreeMap::new(),
        })
    }
}

fn metadata_key(id: TableId) -> Vec<u8> {
    let mut key = META_PREFIX.to_vec();
    key.extend_from_slice(&id.0.to_be_bytes());
    key
}

fn row_table_id(key: &[u8]) -> DomainResult<TableId> {
    if key.len() < 13 || &key[..4] != KEY_MAGIC || key[4] != 1 {
        return Err(DomainError::Corruption("unexpected key namespace or version".into()));
    }
    Ok(TableId(u64::from_le_bytes(key[5..13].try_into().unwrap())))
}

fn make_key(table: &Registered, row: &[Value]) -> DomainResult<Vec<u8>> {
    if row.len() != table.table.columns.len() {
        return Err(DomainError::Table(TableError::Value("row width differs from table schema".into())));
    }
    let values = table.pk_positions.iter().map(|position| row[*position].clone())
        .collect::<Vec<_>>();
    Ok(encode_key(&table.table, &table.pk_positions, &values)?)
}

pub struct MultiTableTransaction<'a> {
    domain: &'a MultiTableStore,
    definitions: BTreeMap<TableId, Registered>,
    base_lsn: u64,
    snapshot: BTreeMap<Vec<u8>, Vec<Value>>,
    changes: BTreeMap<Vec<u8>, Option<Vec<Value>>>,
}

impl MultiTableTransaction<'_> {
    pub fn start_lsn(&self) -> u64 { self.base_lsn }

    fn schema(&self, table: TableId) -> DomainResult<&Registered> {
        self.definitions.get(&table).ok_or(DomainError::UnknownTable(table.0))
    }

    fn visible(&self, key: &[u8]) -> Option<&Vec<Value>> {
        match self.changes.get(key) {
            Some(value) => value.as_ref(),
            None => self.snapshot.get(key),
        }
    }

    pub fn get(&self, table_id: TableId, pk: &[Value]) -> DomainResult<Option<Vec<Value>>> {
        let schema = self.schema(table_id)?;
        let key = encode_key(&schema.table, &schema.pk_positions, pk)?;
        Ok(self.visible(&key).cloned())
    }

    pub fn scan(&self, table_id: TableId) -> DomainResult<Vec<Vec<Value>>> {
        self.schema(table_id)?;
        let mut combined = self.snapshot.iter()
            .filter(|(key,_)| row_table_id(key).ok() == Some(table_id))
            .map(|(key, row)| (key.clone(), row.clone()))
            .collect::<BTreeMap<_,_>>();
        for (key, row) in &self.changes {
            if row_table_id(key)? != table_id { continue; }
            match row {
                Some(row) => { combined.insert(key.clone(), row.clone()); }
                None => { combined.remove(key); }
            }
        }
        Ok(combined.into_values().collect())
    }

    pub fn insert(&mut self, table_id: TableId, row: &[Value]) -> DomainResult<()> {
        let schema = self.schema(table_id)?;
        let encoded = encode_row(&schema.table, row)?;
        if encoded.len() > MAX_INLINE_VALUE {
            return Err(DomainError::Table(TableError::Value("encoded row exceeds inline capacity".into())));
        }
        let key = make_key(schema, row)?;
        if self.visible(&key).is_some() { return Err(DomainError::Table(TableError::DuplicateKey)); }
        self.changes.insert(key, Some(row.to_vec()));
        Ok(())
    }

    pub fn replace(&mut self, table_id: TableId, row: &[Value]) -> DomainResult<()> {
        let schema = self.schema(table_id)?;
        let encoded = encode_row(&schema.table, row)?;
        if encoded.len() > MAX_INLINE_VALUE {
            return Err(DomainError::Table(TableError::Value("encoded row exceeds inline capacity".into())));
        }
        let key = make_key(schema, row)?;
        if self.visible(&key).is_none() { return Err(DomainError::Table(TableError::NotFound)); }
        self.changes.insert(key, Some(row.to_vec()));
        Ok(())
    }

    pub fn delete(&mut self, table_id: TableId, pk: &[Value]) -> DomainResult<()> {
        let schema = self.schema(table_id)?;
        let key = encode_key(&schema.table, &schema.pk_positions, pk)?;
        if self.visible(&key).is_none() { return Err(DomainError::Table(TableError::NotFound)); }
        self.changes.insert(key, None);
        Ok(())
    }

    pub fn rollback(self) {}

    /// The entire cross-table write set is ONE synchronized native WAL batch.
    /// A global LSN conflict prevents stale-snapshot commits; a failed WAL
    /// write poisons the domain until ordinary crash recovery.
    pub fn commit(self) -> DomainResult<u64> {
        if self.changes.is_empty() { return Ok(self.base_lsn); }
        let mut ops = Vec::with_capacity(self.changes.len());
        for (key, value) in self.changes {
            match value {
                Some(row) => {
                    let table_id = row_table_id(&key)?;
                    let table = self.definitions.get(&table_id).ok_or(DomainError::UnknownTable(table_id.0))?;
                    let bytes = encode_row(&table.table, &row)?;
                    ops.push(RowMutation::Put { key, value: bytes });
                }
                None => ops.push(RowMutation::Delete { key }),
            }
        }
        match self.domain.storage.commit_batch(Some(self.base_lsn), &ops) {
            Ok(lsn) => Ok(lsn),
            Err(StoreError::Conflict { .. }) => Err(DomainError::Conflict),
            Err(e) => Err(e.into()),
        }
    }
}
