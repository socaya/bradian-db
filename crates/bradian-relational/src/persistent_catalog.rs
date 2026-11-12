//! WAL-backed, replay-validated Bradian relational metadata catalog.
//!
//! A catalog change is first applied to a private clone of the validated
//! in-memory Catalog, then durably appended as ONE BRWL mutation. Only after
//! WAL sync succeeds does that new metadata become visible. On restart, every
//! DDL operation is replayed through the original Catalog validators.
//!
//! This is an operational local transactional *metadata* journal, not SQL
//! parsing, schema migration of existing rows, MVCC, or multi-table data DDL.
//! The journal is append-only until a future versioned catalog compactor is
//! designed. Its physical store is exclusively owned by one RowStore process.

use std::path::Path;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    Catalog, CatalogError, DatabaseId, ForeignKeySpec, SchemaId, TableId, TableSpec,
};
use crate::row_store::{RowStore, StoreError};

const MARKER_KEY: &[u8] = b"bradian.catalog.version";
const MARKER_VALUE: &[u8] = b"BRADIAN-CATALOG-DDL-V1";
const EVENT_PREFIX: &str = "ddl/";
const JOURNAL_VERSION: u8 = 1;

#[derive(Debug, Error)]
pub enum PersistentCatalogError {
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error(transparent)]
    Storage(#[from] StoreError),
    #[error(transparent)]
    Encoding(#[from] serde_json::Error),
    #[error("catalog journal is corrupt: {0}")]
    Corruption(String),
}

pub type PersistentCatalogResult<T> = Result<T, PersistentCatalogError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Command {
    CreateDatabase { name: String },
    CreateSchema { database: DatabaseId, name: String },
    CreateTable { schema: SchemaId, specification: TableSpec },
    AddForeignKey { table: TableId, specification: ForeignKeySpec },
    DropTable { table: TableId },
    DropSchema { schema: SchemaId },
    DropDatabase { database: DatabaseId },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Event {
    version: u8,
    /// Expected allocated catalog ID for CREATE, or None for other DDL.
    /// Replaying a different ID is an integrity failure, not a migration.
    result_id: Option<u64>,
    command: Command,
}

impl Event {
    fn apply(&self, catalog: &mut Catalog) -> PersistentCatalogResult<Option<u64>> {
        let result = match &self.command {
            Command::CreateDatabase { name } => {
                Some(catalog.create_database(name)?.0)
            }
            Command::CreateSchema { database, name } => {
                Some(catalog.create_schema(*database, name)?.0)
            }
            Command::CreateTable { schema, specification } => {
                Some(catalog.create_table(*schema, specification.clone())?.0)
            }
            Command::AddForeignKey { table, specification } => {
                catalog.add_foreign_key(*table, specification.clone())?;
                None
            }
            Command::DropTable { table } => {
                catalog.drop_table(*table)?;
                None
            }
            Command::DropSchema { schema } => {
                catalog.drop_schema(*schema)?;
                None
            }
            Command::DropDatabase { database } => {
                catalog.drop_database(*database)?;
                None
            }
        };
        if result != self.result_id {
            return Err(PersistentCatalogError::Corruption(
                "replayed identity differs from journaled identity".into(),
            ));
        }
        Ok(result)
    }
}

#[derive(Debug)]
struct Inner {
    catalog: Catalog,
    journal: RowStore,
}

/// One exclusively owned durable catalog. Metadata operations are serialized
/// under a process mutex, then WAL-synced before their new state is published.
#[derive(Debug)]
pub struct PersistentCatalog {
    inner: Mutex<Inner>,
}

impl PersistentCatalog {
    pub fn open(path: impl AsRef<Path>) -> PersistentCatalogResult<Self> {
        let journal = RowStore::open(path)?;
        let mut catalog = Catalog::new();
        let rows = journal.scan();
        if rows.is_empty() {
            journal.put(MARKER_KEY, MARKER_VALUE)?;
        } else {
            if journal.get(MARKER_KEY).as_deref() != Some(MARKER_VALUE) {
                return Err(PersistentCatalogError::Corruption(
                    "catalog version marker is absent or incompatible".into(),
                ));
            }
            // The marker is one WAL mutation; subsequent successful DDL
            // operations consume one sequential WAL LSN each.
            let mut next_lsn = 2_u64;
            for (key, value) in &rows {
                if key.as_slice() == MARKER_KEY {
                    continue;
                }
                let expected_key = event_key(next_lsn);
                if key != expected_key.as_bytes() {
                    return Err(PersistentCatalogError::Corruption(
                        format!("missing, duplicated, or unexpected DDL event at LSN {next_lsn}"),
                    ));
                }
                let event: Event = serde_json::from_slice(value)
                    .map_err(|err| PersistentCatalogError::Corruption(
                        format!("invalid DDL event at LSN {next_lsn}: {err}"),
                    ))?;
                if event.version != JOURNAL_VERSION {
                    return Err(PersistentCatalogError::Corruption(
                        format!("unsupported DDL event version at LSN {next_lsn}"),
                    ));
                }
                event.apply(&mut catalog)
                    .map_err(|err| PersistentCatalogError::Corruption(
                        format!("DDL replay failed at LSN {next_lsn}: {err}"),
                    ))?;
                next_lsn = next_lsn.checked_add(1)
                    .ok_or_else(|| PersistentCatalogError::Corruption(
                        "catalog event sequence overflow".into(),
                    ))?;
            }
            if journal.last_lsn() != next_lsn - 1 {
                return Err(PersistentCatalogError::Corruption(
                    "catalog journal LSN does not match event count".into(),
                ));
            }
        }
        Ok(Self { inner: Mutex::new(Inner { catalog, journal }) })
    }

    /// Return an owned consistent metadata view. A cloned catalog is necessary
    /// because future DDL must not mutate the reader's snapshot.
    pub fn snapshot(&self) -> Catalog {
        self.inner.lock().catalog.clone()
    }

    pub fn last_lsn(&self) -> u64 {
        self.inner.lock().journal.last_lsn()
    }

    /// Publish the catalog journal through Bradian's BRPG checkpoint protocol.
    /// The append-only logical DDL events themselves remain recoverable.
    pub fn checkpoint(&self) -> PersistentCatalogResult<u64> {
        let inner = self.inner.lock();
        Ok(inner.journal.checkpoint()?)
    }

    fn commit(&self, command: Command) -> PersistentCatalogResult<Option<u64>> {
        let mut inner = self.inner.lock();
        let mut tentative = inner.catalog.clone();
        // Validate and allocate stable identities before modifying the WAL.
        let mut event = Event {
            version: JOURNAL_VERSION,
            result_id: None,
            command,
        };
        event.result_id = match &event.command {
            Command::CreateDatabase { name } =>
                Some(tentative.create_database(name)?.0),
            Command::CreateSchema { database, name } =>
                Some(tentative.create_schema(*database, name)?.0),
            Command::CreateTable { schema, specification } =>
                Some(tentative.create_table(*schema, specification.clone())?.0),
            Command::AddForeignKey { table, specification } => {
                tentative.add_foreign_key(*table, specification.clone())?;
                None
            }
            Command::DropTable { table } => {
                tentative.drop_table(*table)?;
                None
            }
            Command::DropSchema { schema } => {
                tentative.drop_schema(*schema)?;
                None
            }
            Command::DropDatabase { database } => {
                tentative.drop_database(*database)?;
                None
            }
        };
        let next_lsn = inner.journal.last_lsn().checked_add(1)
            .ok_or(PersistentCatalogError::Storage(StoreError::SequenceExhausted))?;
        let bytes = serde_json::to_vec(&event)?;
        // Oversized DDL definitions must fail before being committed. RowStore
        // enforces the inline record limit and leaves the WAL unmodified.
        let actual = inner.journal.put(event_key(next_lsn).as_bytes(), &bytes)?;
        if actual != next_lsn {
            return Err(PersistentCatalogError::Corruption(
                "catalog journal recorded unexpected LSN".into(),
            ));
        }
        inner.catalog = tentative;
        Ok(event.result_id)
    }

    pub fn create_database(&self, name: &str) -> PersistentCatalogResult<DatabaseId> {
        Ok(DatabaseId(self.commit(Command::CreateDatabase { name: name.into() })?
            .expect("create allocates identity")))
    }

    pub fn create_schema(
        &self, database: DatabaseId, name: &str,
    ) -> PersistentCatalogResult<SchemaId> {
        Ok(SchemaId(self.commit(Command::CreateSchema { database, name: name.into() })?
            .expect("create allocates identity")))
    }

    pub fn create_table(
        &self, schema: SchemaId, specification: TableSpec,
    ) -> PersistentCatalogResult<TableId> {
        Ok(TableId(self.commit(Command::CreateTable { schema, specification })?
            .expect("create allocates identity")))
    }

    pub fn add_foreign_key(
        &self, table: TableId, specification: ForeignKeySpec,
    ) -> PersistentCatalogResult<()> {
        self.commit(Command::AddForeignKey { table, specification })?;
        Ok(())
    }

    pub fn drop_table(&self, table: TableId) -> PersistentCatalogResult<()> {
        self.commit(Command::DropTable { table })?;
        Ok(())
    }

    pub fn drop_schema(&self, schema: SchemaId) -> PersistentCatalogResult<()> {
        self.commit(Command::DropSchema { schema })?;
        Ok(())
    }

    pub fn drop_database(&self, database: DatabaseId) -> PersistentCatalogResult<()> {
        self.commit(Command::DropDatabase { database })?;
        Ok(())
    }
}

fn event_key(lsn: u64) -> String {
    // Fixed-width hexadecimal preserves the commit order within the RowStore
    // ordered binary namespace.
    format!("{EVENT_PREFIX}{lsn:016x}")
}
