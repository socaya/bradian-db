//! Generic, domain-independent relational schema catalog for Bradian Database Systems.
//!
//! This is a **schema/catalog foundation only**. It is not a SQL parser, a durable
//! transaction manager, or an authoritative row store. Metadata persistence and
//! crash-safe DDL require a separate design and release gate.
//!
//! Names currently follow a conservative case-insensitive unquoted ASCII profile.
//! Quoted identifiers, Unicode collations and SQL compatibility remain future work.

#![forbid(unsafe_code)]

mod model;
mod catalog;
pub mod page;
pub mod row_store;

pub use catalog::{Catalog, CatalogError};
pub use model::{
    Column, DataType, Database, DatabaseId, ForeignKey, ForeignKeySpec, PrimaryKey,
    Schema, SchemaId, Table, TableId, TableSpec, UniqueKey,
};
