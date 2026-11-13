use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
use crate::model::*;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CatalogError {
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(String),
    #[error("duplicate object: {0}")]
    Duplicate(String),
    #[error("object not found: {0}")]
    NotFound(String),
    #[error("invalid definition: {0}")]
    InvalidDefinition(String),
    #[error("object has dependent objects: {0}")]
    DependentObjects(String),
    #[error("catalog identity space exhausted")]
    IdentityExhausted,
}

/// An isolated relational schema catalog. Mutation methods validate everything
/// before publishing new objects. Concurrency, persistence and transactional DDL
/// are not provided by this first milestone.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    next_id: u64,
    databases: BTreeMap<DatabaseId, Database>,
    schemas: BTreeMap<SchemaId, Schema>,
    tables: BTreeMap<TableId, Table>,
}

fn identifier(name: &str) -> Result<String, CatalogError> {
    let valid = !name.is_empty()
        && name.len() <= 63
        && name.is_ascii()
        && name.bytes().next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if !valid {
        return Err(CatalogError::InvalidIdentifier(name.to_owned()));
    }
    Ok(name.to_ascii_lowercase())
}

fn names(columns: &[String], existing: &BTreeSet<String>) -> Result<Vec<String>, CatalogError> {
    if columns.is_empty() {
        return Err(CatalogError::InvalidDefinition("key must contain at least one column".into()));
    }
    let mut seen = BTreeSet::new();
    let mut canonical = Vec::with_capacity(columns.len());
    for col in columns {
        let col = identifier(col)?;
        if !existing.contains(&col) {
            return Err(CatalogError::NotFound(format!("column {col}")));
        }
        if !seen.insert(col.clone()) {
            return Err(CatalogError::Duplicate(format!("key column {col}")));
        }
        canonical.push(col);
    }
    Ok(canonical)
}

pub(crate) fn validate_type(data_type: &DataType) -> Result<(), CatalogError> {
    match data_type {
        DataType::Decimal { precision, scale } if *precision == 0 || *precision > 38 || scale > precision =>
            Err(CatalogError::InvalidDefinition("decimal precision must be 1..=38 and scale <= precision".into())),
        DataType::Varchar { max_length } if *max_length == 0 || *max_length > 65535 =>
            Err(CatalogError::InvalidDefinition("varchar length must be 1..=65535".into())),
        _ => Ok(()),
    }
}

pub(crate) fn validate_revision_columns(table: &Table) -> Result<(), CatalogError> {
    for column in &table.columns {
        if identifier(&column.name)? != column.name {
            return Err(CatalogError::InvalidIdentifier(column.name.clone()));
        }
        validate_type(&column.data_type)?;
    }
    Ok(())
}

impl Catalog {
    pub fn new() -> Self {
        Self { next_id: 1, ..Self::default() }
    }

    fn allocate(&mut self) -> Result<u64, CatalogError> {
        let id = if self.next_id == 0 { 1 } else { self.next_id };
        self.next_id = id.checked_add(1).ok_or(CatalogError::IdentityExhausted)?;
        Ok(id)
    }

    pub fn databases(&self) -> impl Iterator<Item = &Database> {
        self.databases.values()
    }
    pub fn schemas(&self) -> impl Iterator<Item = &Schema> {
        self.schemas.values()
    }
    pub fn tables(&self) -> impl Iterator<Item = &Table> {
        self.tables.values()
    }
    pub fn database(&self, id: DatabaseId) -> Option<&Database> { self.databases.get(&id) }
    pub fn schema(&self, id: SchemaId) -> Option<&Schema> { self.schemas.get(&id) }
    pub fn table(&self, id: TableId) -> Option<&Table> { self.tables.get(&id) }

    pub fn create_database(&mut self, name: &str) -> Result<DatabaseId, CatalogError> {
        let name = identifier(name)?;
        if self.databases.values().any(|db| db.name == name) {
            return Err(CatalogError::Duplicate(format!("database {name}")));
        }
        let id = DatabaseId(self.allocate()?);
        self.databases.insert(id, Database { id, name });
        Ok(id)
    }

    pub fn create_schema(&mut self, database: DatabaseId, name: &str) -> Result<SchemaId, CatalogError> {
        if !self.databases.contains_key(&database) {
            return Err(CatalogError::NotFound(format!("database {}", database.0)));
        }
        let name = identifier(name)?;
        if self.schemas.values().any(|s| s.database == database && s.name == name) {
            return Err(CatalogError::Duplicate(format!("schema {name}")));
        }
        let id = SchemaId(self.allocate()?);
        self.schemas.insert(id, Schema { id, database, name });
        Ok(id)
    }

    pub fn create_table(&mut self, schema: SchemaId, spec: TableSpec) -> Result<TableId, CatalogError> {
        if !self.schemas.contains_key(&schema) {
            return Err(CatalogError::NotFound(format!("schema {}", schema.0)));
        }
        let table_name = identifier(&spec.name)?;
        if self.tables.values().any(|t| t.schema == schema && t.name == table_name) {
            return Err(CatalogError::Duplicate(format!("table {table_name}")));
        }
        if spec.columns.is_empty() {
            return Err(CatalogError::InvalidDefinition("table requires at least one column".into()));
        }
        let mut columns = Vec::with_capacity(spec.columns.len());
        let mut colnames = BTreeSet::new();
        for mut col in spec.columns {
            col.name = identifier(&col.name)?;
            validate_type(&col.data_type)?;
            if !colnames.insert(col.name.clone()) {
                return Err(CatalogError::Duplicate(format!("column {}", col.name)));
            }
            columns.push(col);
        }
        let pk = names(&spec.primary_key.columns, &colnames)?;
        for col in &columns {
            if pk.contains(&col.name) && col.nullable {
                return Err(CatalogError::InvalidDefinition(format!("primary key column {} must be NOT NULL", col.name)));
            }
        }
        let mut unique_names = BTreeSet::new();
        let mut unique_keys = Vec::new();
        for mut uk in spec.unique_keys {
            uk.name = identifier(&uk.name)?;
            if !unique_names.insert(uk.name.clone()) {
                return Err(CatalogError::Duplicate(format!("constraint {}", uk.name)));
            }
            uk.columns = names(&uk.columns, &colnames)?;
            unique_keys.push(uk);
        }
        let id = TableId(self.allocate()?);
        self.tables.insert(id, Table {
            id, schema, name: table_name, columns,
            primary_key: PrimaryKey { columns: pk },
            unique_keys, foreign_keys: Vec::new(), column_defaults: BTreeMap::new(),
        });
        Ok(id)
    }

    pub fn add_foreign_key(&mut self, source_id: TableId, spec: ForeignKeySpec) -> Result<(), CatalogError> {
        let source = self.tables.get(&source_id)
            .ok_or_else(|| CatalogError::NotFound(format!("table {}", source_id.0)))?;
        let target = self.tables.get(&spec.referenced_table)
            .ok_or_else(|| CatalogError::NotFound(format!("table {}", spec.referenced_table.0)))?;
        let source_db = self.schemas.get(&source.schema).expect("catalog schema").database;
        let target_db = self.schemas.get(&target.schema).expect("catalog schema").database;
        if source_db != target_db {
            return Err(CatalogError::InvalidDefinition("cross-database foreign keys are unsupported".into()));
        }
        let constraint_name = identifier(&spec.name)?;
        if source.foreign_keys.iter().any(|fk| fk.name == constraint_name)
            || source.unique_keys.iter().any(|uk| uk.name == constraint_name)
        {
            return Err(CatalogError::Duplicate(format!("constraint {constraint_name}")));
        }
        let source_cols = source.columns.iter().map(|c| c.name.clone()).collect::<BTreeSet<_>>();
        let target_cols = target.columns.iter().map(|c| c.name.clone()).collect::<BTreeSet<_>>();
        let columns = names(&spec.columns, &source_cols)?;
        let referenced_columns = names(&spec.referenced_columns, &target_cols)?;
        if columns.len() != referenced_columns.len() {
            return Err(CatalogError::InvalidDefinition("foreign key arity mismatch".into()));
        }
        let candidate = target.primary_key.columns == referenced_columns
            || target.unique_keys.iter().any(|uk| uk.columns == referenced_columns
                && uk.columns.iter().all(|name| target.columns.iter().any(|c| c.name == *name && !c.nullable)));
        if !candidate {
            return Err(CatalogError::InvalidDefinition("foreign key target must match a primary key or non-null unique key".into()));
        }
        for (local, remote) in columns.iter().zip(referenced_columns.iter()) {
            let l = source.columns.iter().find(|c| &c.name == local).expect("validated column");
            let r = target.columns.iter().find(|c| &c.name == remote).expect("validated column");
            if l.data_type != r.data_type {
                return Err(CatalogError::InvalidDefinition("foreign key types differ".into()));
            }
        }
        self.tables.get_mut(&source_id).expect("validated table")
            .foreign_keys.push(ForeignKey {
                name: constraint_name,
                columns,
                referenced_table: spec.referenced_table,
                referenced_columns,
            });
        Ok(())
    }

    /// Replace metadata only; row-domain coordination belongs to the owning engine.
    pub(crate) fn activate_nullable_revision(&mut self, proposed: &Table) -> Result<(), CatalogError> {
        let current = self.table(proposed.id)
            .ok_or_else(|| CatalogError::NotFound(format!("table {}", proposed.id.0)))?;
        crate::schema_evolution::plan_schema_projection(current, proposed)
            .map_err(|err| CatalogError::InvalidDefinition(err.to_string()))?;
        validate_revision_columns(proposed)?;
        self.tables.insert(proposed.id, proposed.clone());
        Ok(())
    }

    /// RESTRICT semantics: never silently remove a referenced relation.
    pub fn drop_table(&mut self, id: TableId) -> Result<(), CatalogError> {
        if !self.tables.contains_key(&id) {
            return Err(CatalogError::NotFound(format!("table {}", id.0)));
        }
        if self.tables.values().any(|t| t.id != id
            && t.foreign_keys.iter().any(|fk| fk.referenced_table == id))
        {
            return Err(CatalogError::DependentObjects(format!("table {}", id.0)));
        }
        self.tables.remove(&id);
        Ok(())
    }

    pub fn drop_schema(&mut self, id: SchemaId) -> Result<(), CatalogError> {
        if !self.schemas.contains_key(&id) {
            return Err(CatalogError::NotFound(format!("schema {}", id.0)));
        }
        if self.tables.values().any(|t| t.schema == id) {
            return Err(CatalogError::DependentObjects(format!("schema {}", id.0)));
        }
        self.schemas.remove(&id);
        Ok(())
    }

    pub fn drop_database(&mut self, id: DatabaseId) -> Result<(), CatalogError> {
        if !self.databases.contains_key(&id) {
            return Err(CatalogError::NotFound(format!("database {}", id.0)));
        }
        if self.schemas.values().any(|s| s.database == id) {
            return Err(CatalogError::DependentObjects(format!("database {}", id.0)));
        }
        self.databases.remove(&id);
        Ok(())
    }
}
