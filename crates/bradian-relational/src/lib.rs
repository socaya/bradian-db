//! Generic, domain-independent relational schema catalog for Bradian Database Systems.
//!
//! This is a **schema/catalog foundation only**. It is not a SQL parser, a durable
//! transaction manager, or an authoritative row store. Metadata persistence and
//! crash-safe DDL require a separate design and release gate.
//!
//! Names currently follow a conservative case-insensitive unquoted ASCII profile.
//! Quoted identifiers, Unicode collations and SQL compatibility remain future work.

// CLAUDE.md §4.9 allows narrow unsafe in clearly-scoped sites with SAFETY
// comments. `deny` lets individual call sites opt in with
// `#[allow(unsafe_code)]`; `forbid` would block them even at the call site.
#![deny(unsafe_code)]

mod metadata_io;
mod model;
mod catalog;
pub mod page;
pub mod row_store;
pub mod btree;
pub mod buffer_pool;
pub mod index_recovery;
pub mod checkpoint_manifest;
pub mod backup;
pub mod typed_table;
pub mod persistent_catalog;
pub mod sql;
pub mod transaction_domain;
pub mod domain_sql;
pub mod legacy_migration;
pub mod lifecycle;
pub mod database_backup;
pub mod mvcc;
pub mod typed_mvcc;
pub mod mvcc_sql;
pub mod durable_mvcc;
pub mod runtime_limits;
pub mod durable_typed;
pub mod history_sql;
mod sql_expression;
mod select_window;
pub mod history_migration;
pub mod history_backup;
pub mod lifecycle_history_migration;
pub mod schema_evolution;
pub mod epoch_row;
pub mod schema_activation;
pub mod local_access;
pub mod workspace_usage;
pub mod mutable_page;
pub mod unique_check;
pub mod pitr;
pub mod collation;
pub mod audit_log;
pub mod audit_chain;
pub mod lock_policy;
pub mod format_manifest;
pub mod metrics;
pub mod slow_query_log;
pub mod catalog_compactor;
pub mod cdc;
pub mod incremental_backup;
pub mod windows_lock;
pub mod access_policy;
/// ADR 0127 — in-memory non-unique secondary index helper.
pub mod secondary_index;
/// ADR 0128 — HTAP freshness watermark primitive.
pub mod htap_watermark;
/// ADR 0129 — local driver facade trait.
pub mod driver;
/// ADR 0131 — bounded reader fuzzing harness.
pub mod reader_fuzz;
pub mod bace;

pub use catalog::{Catalog, CatalogError, Dependency};
pub use model::{
    Column, DataType, Database, DatabaseId, ForeignKey, ForeignKeySpec, PrimaryKey,
    Schema, SchemaId, Table, TableId, TableSpec, UniqueKey,
};
