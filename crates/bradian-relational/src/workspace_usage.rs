//! Observational workspace usage snapshot (ADR 0059).
//!
//! Combines the already-exposed per-engine usage accessors into a single
//! operator-facing snapshot. **Non-admitting.** This is purely for
//! diagnostics; it does not change behavior, does not retain references,
//! and does not block any admission path. Each `with_*` call takes one
//! atomic reading from the engine under its own lock.

use crate::durable_mvcc::DurableMvccStore;
use crate::mvcc::MvccStore;
use crate::persistent_catalog::PersistentCatalog;
use crate::row_store::RowStore;
use crate::runtime_limits::{CatalogUsage, HistoryUsage, RowStoreUsage};
use crate::transaction_domain::MultiTableStore;

/// A one-shot combined view of decoded-state usage across the catalog
/// and the opt-in engines an operator might have attached. Catalog
/// counts are always present; every other field is `None` when the
/// operator has not yet composed that engine into the snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceUsage {
    pub catalog: CatalogUsage,
    pub domain_tables: Option<usize>,
    pub mvcc_distinct_keys: Option<usize>,
    pub mvcc_total_versions: Option<usize>,
    /// ADR 0059 extension — RowStore decoded-state usage (rows,
    /// bytes, delta keys/bytes). Separate from `domain_tables`:
    /// operators may attach a standalone RowStore.
    pub row_store: Option<RowStoreUsage>,
    /// ADR 0059 extension — DurableMvccStore retained-history usage
    /// (keys, versions, bytes). Separate from `mvcc_*` which is the
    /// in-memory MvccStore.
    pub durable_mvcc: Option<HistoryUsage>,
}

impl WorkspaceUsage {
    /// Start a snapshot from a catalog. This takes one atomic reading of
    /// the catalog's current decoded counts.
    pub fn for_catalog(catalog: &PersistentCatalog) -> Self {
        Self { catalog: catalog.resource_usage(), ..Self::default() }
    }

    /// Add the domain's registered-table count. Takes one atomic reading.
    pub fn with_domain(mut self, domain: &MultiTableStore) -> Self {
        self.domain_tables = Some(domain.registered_table_count());
        self
    }

    /// Add the opt-in MVCC store's retained distinct keys and total
    /// version count. Takes one atomic reading from `MvccStore::stats`.
    pub fn with_mvcc(mut self, mvcc: &MvccStore) -> Self {
        let stats = mvcc.stats();
        self.mvcc_distinct_keys = Some(stats.keys_retained);
        self.mvcc_total_versions = Some(stats.versions_retained);
        self
    }

    /// Add a RowStore's `resource_usage()` snapshot. One atomic reading
    /// under the store's own mutex.
    pub fn with_row_store(mut self, store: &RowStore) -> Self {
        self.row_store = Some(store.resource_usage());
        self
    }

    /// Add a DurableMvccStore's retained-history usage. One atomic
    /// reading under the store's own state lock.
    pub fn with_durable_mvcc(mut self, store: &DurableMvccStore) -> Self {
        self.durable_mvcc = Some(store.history_usage());
        self
    }
}

impl std::fmt::Display for WorkspaceUsage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "catalog: {} dbs / {} schemas / {} tables / {} revisions",
            self.catalog.databases,
            self.catalog.schemas,
            self.catalog.tables,
            self.catalog.revisions,
        )?;
        if let Some(n) = self.domain_tables {
            write!(f, "; domain: {n} tables")?;
        }
        if let (Some(k), Some(v)) = (self.mvcc_distinct_keys, self.mvcc_total_versions) {
            write!(f, "; mvcc: {k} keys / {v} versions")?;
        }
        if let Some(u) = self.row_store {
            write!(f, "; row_store: {} rows / {} bytes / {} delta-keys / {} delta-bytes",
                u.rows, u.row_bytes, u.delta_keys, u.delta_bytes)?;
        }
        if let Some(h) = self.durable_mvcc {
            write!(f, "; durable_mvcc: {} keys / {} versions / {} bytes",
                h.keys, h.versions, h.bytes)?;
        }
        Ok(())
    }
}
