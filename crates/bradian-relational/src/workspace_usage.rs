//! Observational workspace usage snapshot (ADR 0059).
//!
//! Combines the already-exposed per-engine usage accessors into a single
//! operator-facing snapshot. **Non-admitting.** This is purely for
//! diagnostics; it does not change behavior, does not retain references,
//! and does not block any admission path. Each `with_*` call takes one
//! atomic reading from the engine under its own lock.

use crate::mvcc::MvccStore;
use crate::persistent_catalog::PersistentCatalog;
use crate::runtime_limits::CatalogUsage;
use crate::transaction_domain::MultiTableStore;

/// A one-shot combined view of decoded-state usage across the catalog
/// and the opt-in engines an operator might have attached. Catalog
/// counts are always present; domain and MVCC fields are `None` when
/// the operator has not yet composed those engines into the snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceUsage {
    pub catalog: CatalogUsage,
    pub domain_tables: Option<usize>,
    pub mvcc_distinct_keys: Option<usize>,
    pub mvcc_total_versions: Option<usize>,
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
        Ok(())
    }
}
