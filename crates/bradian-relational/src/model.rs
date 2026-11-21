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
    /// ADR 0113 — JSON document stored as canonical UTF-8;
    /// validated by serde_json at the application layer.
    Json,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub data_type: DataType,
    pub nullable: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimaryKey {
    pub columns: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UniqueKey {
    pub name: String,
    pub columns: Vec<String>,
}

/// ADR 0182 — simple per-column CHECK operator. The RHS is a
/// stored literal; the LHS is implicitly the column the
/// constraint is attached to. Evaluated at INSERT and UPDATE.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CheckOp {
    /// Human-readable form used in error messages.
    pub fn sql(self) -> &'static str {
        match self {
            CheckOp::Eq => "=", CheckOp::Ne => "<>",
            CheckOp::Lt => "<", CheckOp::Le => "<=",
            CheckOp::Gt => ">", CheckOp::Ge => ">=",
        }
    }
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableSpec {
    pub name: String,
    pub columns: Vec<Column>,
    pub primary_key: PrimaryKey,
    pub unique_keys: Vec<UniqueKey>,
    /// ADR 0175 — optional per-column collation declared in
    /// CREATE TABLE via `COLLATE <name>`. Only Text / Varchar
    /// columns may carry a non-default collation; the parser
    /// rejects COLLATE on other types. Empty map preserves
    /// pre-ADR-0175 TableSpec bytes.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub column_collations: std::collections::BTreeMap<String, crate::collation::Collation>,
    /// ADR 0182 — per-column CHECK predicates. One column may
    /// carry multiple checks; all must pass. Empty map preserves
    /// pre-ADR-0182 TableSpec bytes.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub column_checks: std::collections::BTreeMap<String, Vec<(CheckOp, crate::typed_table::Value)>>,
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
    /// ADR 0113 — per-column collation choice. Empty map
    /// preserves pre-ADR-0113 natural type ordering. Keyed by
    /// column name so adding/removing columns is tolerant.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub column_collations: std::collections::BTreeMap<String, crate::collation::Collation>,
    /// ADR 0157 — named row/column/tenant policies attached
    /// to this table. Keyed by policy name. Empty map
    /// preserves pre-ADR-0157 schema bytes.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub policies: std::collections::BTreeMap<String, crate::access_policy::TableAccessPolicy>,
    /// ADR 0182 — per-column CHECK predicates persisted on the
    /// durable catalog. Evaluated at INSERT / UPDATE by every
    /// SQL engine. Empty map preserves pre-ADR-0182 bytes.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub column_checks: std::collections::BTreeMap<String, Vec<(CheckOp, crate::typed_table::Value)>>,
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
