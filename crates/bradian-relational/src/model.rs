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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignKeySpec {
    pub name: String,
    pub columns: Vec<String>,
    pub referenced_table: TableId,
    pub referenced_columns: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Epoch-bound constants. Omitting empty maps preserves legacy schema bytes.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub column_defaults: std::collections::BTreeMap<String, crate::typed_table::Value>,
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;

    #[test]
    fn legacy_empty_default_schema_keeps_exact_canonical_bytes() {
        let legacy = r#"{"id":8,"schema":2,"name":"entries","columns":[{"name":"id","data_type":"Int64","nullable":false}],"primary_key":{"columns":["id"]},"unique_keys":[],"foreign_keys":[]}"#;
        let table: Table = serde_json::from_str(legacy).unwrap();
        assert!(table.column_defaults.is_empty());
        assert_eq!(serde_json::to_vec(&table).unwrap(), legacy.as_bytes());
    }
}
