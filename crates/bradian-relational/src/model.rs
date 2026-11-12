use serde::{Deserialize, Serialize};

/// Stable catalog object identities are allocated monotonically within one catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DatabaseId(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SchemaId(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TableId(pub u64);

/// Minimal validated relational data types; SQL precision and runtime value checks
/// will be implemented by the transaction engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataType {
    Boolean,
    Int32,
    Int64,
    Decimal { precision: u8, scale: u8 },
    Varchar { max_length: u32 },
    Text,
    TimestampTz,
    Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub data_type: DataType,
    pub nullable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimaryKey {
    pub columns: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UniqueKey {
    pub name: String,
    pub columns: Vec<String>,
}

/// Target table and columns are resolved to stable object identifiers by the catalog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignKey {
    pub name: String,
    pub columns: Vec<String>,
    pub referenced_table: TableId,
    pub referenced_columns: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForeignKeySpec {
    pub name: String,
    pub columns: Vec<String>,
    pub referenced_table: TableId,
    pub referenced_columns: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableSpec {
    pub name: String,
    pub columns: Vec<Column>,
    pub primary_key: PrimaryKey,
    pub unique_keys: Vec<UniqueKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Database {
    pub id: DatabaseId,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schema {
    pub id: SchemaId,
    pub database: DatabaseId,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    pub id: TableId,
    pub schema: SchemaId,
    pub name: String,
    pub columns: Vec<Column>,
    pub primary_key: PrimaryKey,
    pub unique_keys: Vec<UniqueKey>,
    pub foreign_keys: Vec<ForeignKey>,
}
