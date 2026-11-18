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
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
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

/// ADR 0126 — one entry per dependency edge that blocks a
/// DDL mutation. Returned by `table_dependents` /
/// `column_dependents` so callers can report every reason
/// at once, not just the first refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dependency {
    /// Dropping the only column would leave the table empty.
    LastColumn,
    /// Column is part of the table's primary key.
    PrimaryKey { columns: Vec<String> },
    /// Column is part of a declared unique constraint.
    UniqueKey { constraint: String, columns: Vec<String> },
    /// Column is part of a local foreign key.
    ForeignKey {
        constraint: String,
        columns: Vec<String>,
        referenced_table: TableId,
    },
    /// Table or column is referenced by a foreign key in
    /// another table (incoming reference).
    IncomingForeignKey {
        from_table: TableId,
        constraint: String,
        columns: Vec<String>,
        referenced_columns: Vec<String>,
    },
}

/// ADR 0130 — strictly additive widenings only.
fn is_widening(from: &DataType, to: &DataType) -> bool {
    match (from, to) {
        (DataType::Int32, DataType::Int64) => true,
        (DataType::Varchar { max_length: a }, DataType::Varchar { max_length: b }) => b > a,
        (DataType::Varchar { .. }, DataType::Text) => true,
        (DataType::Decimal { precision: p1, scale: s1 },
         DataType::Decimal { precision: p2, scale: s2 }) => p2 > p1 && s1 == s2,
        _ => false,
    }
}

/// ADR 0124 — permissive type-matching for column DEFAULTs.
/// Must stay a *subset* of `typed_table::validate_value`;
/// NULLs are rejected (SET DEFAULT NULL = DROP DEFAULT).
fn default_matches_type(ty: &DataType, v: &crate::typed_table::Value) -> bool {
    use crate::typed_table::Value;
    match (ty, v) {
        (_, Value::Null) => false,
        (DataType::Boolean, Value::Boolean(_)) => true,
        (DataType::Int32, Value::Int32(_)) => true,
        (DataType::Int64, Value::Int64(_)) => true,
        (DataType::Text, Value::String(_)) => true,
        (DataType::TimestampTz, Value::TimestampTz(_)) => true,
        (DataType::Uuid, Value::Uuid(_)) => true,
        (DataType::Varchar { max_length }, Value::String(s)) =>
            s.len() <= *max_length as usize,
        (DataType::Decimal { precision, scale }, Value::Decimal(d)) => {
            if *precision == 0 || *precision > 38 || *scale > *precision { false }
            else {
                let mag = d.unsigned_abs();
                mag < 10_u128.pow(*precision as u32)
            }
        }
        (DataType::Json, Value::String(s)) =>
            serde_json::from_str::<serde_json::Value>(s).is_ok(),
        _ => false,
    }
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
            unique_keys, foreign_keys: Vec::new(),
            column_defaults: BTreeMap::new(),
            column_collations: BTreeMap::new(),
            policies: BTreeMap::new(),
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

    /// ADR 0124 — in-memory rename of a table. Preserves id;
    /// enforces identifier shape + per-schema uniqueness.
    /// PersistentCatalog journaling and HistorySqlEngine
    /// wiring remain OPEN.
    pub fn rename_table(&mut self, id: TableId, new_name: &str) -> Result<(), CatalogError> {
        let new_name = identifier(new_name)?;
        let schema = self.tables.get(&id)
            .ok_or_else(|| CatalogError::NotFound(format!("table {}", id.0)))?
            .schema;
        if self.tables.values().any(|t| t.id != id && t.schema == schema && t.name == new_name) {
            return Err(CatalogError::Duplicate(format!("table {new_name}")));
        }
        self.tables.get_mut(&id).expect("validated table").name = new_name;
        Ok(())
    }

    /// ADR 0124 — in-memory rename of a column. Updates all
    /// references (PK, UKs, FKs, defaults, collations) +
    /// *incoming* FK referenced_columns in dependent tables.
    pub fn rename_column(&mut self, id: TableId, old: &str, new: &str) -> Result<(), CatalogError> {
        let old = identifier(old)?;
        let new = identifier(new)?;
        if old == new { return Ok(()); }
        let table = self.tables.get(&id)
            .ok_or_else(|| CatalogError::NotFound(format!("table {}", id.0)))?;
        if !table.columns.iter().any(|c| c.name == old) {
            return Err(CatalogError::NotFound(format!("column {old}")));
        }
        if table.columns.iter().any(|c| c.name == new) {
            return Err(CatalogError::Duplicate(format!("column {new}")));
        }
        // Mutate the target table.
        let t = self.tables.get_mut(&id).expect("validated table");
        for c in &mut t.columns { if c.name == old { c.name.clone_from(&new); } }
        for col in &mut t.primary_key.columns { if *col == old { col.clone_from(&new); } }
        for uk in &mut t.unique_keys {
            for col in &mut uk.columns { if *col == old { col.clone_from(&new); } }
        }
        for fk in &mut t.foreign_keys {
            for col in &mut fk.columns { if *col == old { col.clone_from(&new); } }
        }
        if let Some(v) = t.column_defaults.remove(&old) { t.column_defaults.insert(new.clone(), v); }
        if let Some(v) = t.column_collations.remove(&old) { t.column_collations.insert(new.clone(), v); }
        // Fix incoming FKs that reference this table's renamed column.
        for other in self.tables.values_mut() {
            if other.id == id { continue; }
            for fk in &mut other.foreign_keys {
                if fk.referenced_table == id {
                    for c in &mut fk.referenced_columns { if *c == old { c.clone_from(&new); } }
                }
            }
        }
        Ok(())
    }

    /// ADR 0124 — in-memory DROP COLUMN. Rejects PK columns,
    /// columns referenced by any UK/FK (local or incoming),
    /// and columns that would leave the table empty.
    pub fn drop_column(&mut self, id: TableId, name: &str) -> Result<(), CatalogError> {
        let name = identifier(name)?;
        let table = self.tables.get(&id)
            .ok_or_else(|| CatalogError::NotFound(format!("table {}", id.0)))?;
        if !table.columns.iter().any(|c| c.name == name) {
            return Err(CatalogError::NotFound(format!("column {name}")));
        }
        if table.columns.len() == 1 {
            return Err(CatalogError::InvalidDefinition("cannot drop last column".into()));
        }
        if table.primary_key.columns.iter().any(|c| c == &name) {
            return Err(CatalogError::DependentObjects(format!("column {name} is in primary key")));
        }
        if table.unique_keys.iter().any(|uk| uk.columns.iter().any(|c| c == &name)) {
            return Err(CatalogError::DependentObjects(format!("column {name} is in a unique key")));
        }
        if table.foreign_keys.iter().any(|fk| fk.columns.iter().any(|c| c == &name)) {
            return Err(CatalogError::DependentObjects(format!("column {name} is in a foreign key")));
        }
        for other in self.tables.values() {
            if other.id == id { continue; }
            if other.foreign_keys.iter().any(|fk| fk.referenced_table == id
                && fk.referenced_columns.iter().any(|c| c == &name))
            {
                return Err(CatalogError::DependentObjects(
                    format!("column {name} is referenced by an incoming foreign key")));
            }
        }
        let t = self.tables.get_mut(&id).expect("validated table");
        t.columns.retain(|c| c.name != name);
        t.column_defaults.remove(&name);
        t.column_collations.remove(&name);
        Ok(())
    }

    /// ADR 0124 — SET DEFAULT on a column. Rejects unknown
    /// columns. Validates the default against the column's
    /// declared type via typed_table::validate_value.
    pub fn set_column_default(&mut self, id: TableId, name: &str, value: crate::typed_table::Value)
        -> Result<(), CatalogError>
    {
        let name = identifier(name)?;
        let table = self.tables.get(&id)
            .ok_or_else(|| CatalogError::NotFound(format!("table {}", id.0)))?;
        let col = table.columns.iter().find(|c| c.name == name)
            .ok_or_else(|| CatalogError::NotFound(format!("column {name}")))?;
        if !default_matches_type(&col.data_type, &value) {
            return Err(CatalogError::InvalidDefinition(
                format!("default for {name} does not match declared type")));
        }
        self.tables.get_mut(&id).expect("validated table")
            .column_defaults.insert(name, value);
        Ok(())
    }

    /// ADR 0126 — Dependency planner: list every catalog
    /// dependency edge that would block DROP TABLE on `id`.
    /// Returns an empty vec iff `drop_table(id)` would
    /// succeed. The planner is pure — no mutation.
    pub fn table_dependents(&self, id: TableId) -> Vec<Dependency> {
        let mut out = Vec::new();
        if !self.tables.contains_key(&id) { return out; }
        for other in self.tables.values() {
            if other.id == id { continue; }
            for fk in &other.foreign_keys {
                if fk.referenced_table == id {
                    out.push(Dependency::IncomingForeignKey {
                        from_table: other.id,
                        constraint: fk.name.clone(),
                        columns: fk.columns.clone(),
                        referenced_columns: fk.referenced_columns.clone(),
                    });
                }
            }
        }
        out
    }

    /// ADR 0126 — Dependency planner for DROP COLUMN. Lists
    /// every reason `drop_column(id, name)` would refuse.
    /// Returns empty iff the drop would succeed. The
    /// planner is pure.
    pub fn column_dependents(&self, id: TableId, name: &str) -> Vec<Dependency> {
        let mut out = Vec::new();
        let name = match identifier(name) { Ok(n) => n, Err(_) => return out };
        let Some(table) = self.tables.get(&id) else { return out; };
        if !table.columns.iter().any(|c| c.name == name) { return out; }
        if table.columns.len() == 1 {
            out.push(Dependency::LastColumn);
        }
        if table.primary_key.columns.iter().any(|c| c == &name) {
            out.push(Dependency::PrimaryKey {
                columns: table.primary_key.columns.clone(),
            });
        }
        for uk in &table.unique_keys {
            if uk.columns.iter().any(|c| c == &name) {
                out.push(Dependency::UniqueKey {
                    constraint: uk.name.clone(),
                    columns: uk.columns.clone(),
                });
            }
        }
        for fk in &table.foreign_keys {
            if fk.columns.iter().any(|c| c == &name) {
                out.push(Dependency::ForeignKey {
                    constraint: fk.name.clone(),
                    columns: fk.columns.clone(),
                    referenced_table: fk.referenced_table,
                });
            }
        }
        for other in self.tables.values() {
            if other.id == id { continue; }
            for fk in &other.foreign_keys {
                if fk.referenced_table == id
                    && fk.referenced_columns.iter().any(|c| c == &name)
                {
                    out.push(Dependency::IncomingForeignKey {
                        from_table: other.id,
                        constraint: fk.name.clone(),
                        columns: fk.columns.clone(),
                        referenced_columns: fk.referenced_columns.clone(),
                    });
                }
            }
        }
        out
    }

    /// ADR 0130 — widen a column's declared type. Allowed
    /// widenings:
    /// - `Int32 -> Int64`
    /// - `Varchar(N) -> Varchar(M)` where `M > N`
    /// - `Varchar(N) -> Text`
    /// - `Decimal(p, s) -> Decimal(p', s)` where `p' > p` and
    ///   the scale is *unchanged* (reinterpreting scale
    ///   would move the stored decimal point).
    ///
    /// Narrowing (`Int64 -> Int32`, shorter Varchar, lower
    /// Decimal precision, scale change) is REJECTED.
    /// Same-type is a no-op (`Ok(())`).
    ///
    /// Rejects widening on PK, UK, FK, incoming-FK columns
    /// (those affect comparison semantics and need the
    /// dependency planner first — ADR 0126).
    pub fn widen_column_type(
        &mut self,
        id: TableId,
        column: &str,
        new_type: DataType,
    ) -> Result<(), CatalogError> {
        let column = identifier(column)?;
        let table = self.tables.get(&id)
            .ok_or_else(|| CatalogError::NotFound(format!("table {}", id.0)))?;
        let col = table.columns.iter().find(|c| c.name == column)
            .ok_or_else(|| CatalogError::NotFound(format!("column {column}")))?;
        if col.data_type == new_type { return Ok(()); }
        if !is_widening(&col.data_type, &new_type) {
            return Err(CatalogError::InvalidDefinition(format!(
                "widen_column_type: {:?} -> {:?} is not a permitted widening",
                col.data_type, new_type)));
        }
        // Reject on any catalog dependency edge — widening
        // under an active FK/UK could change comparison
        // semantics in incompatible ways.
        let deps = self.column_dependents(id, &column);
        if !deps.is_empty() {
            return Err(CatalogError::DependentObjects(format!(
                "column {column} has {} dependency edge(s); widen requires removing them first",
                deps.len())));
        }
        validate_type(&new_type)?;
        let t = self.tables.get_mut(&id).expect("validated table");
        for c in &mut t.columns {
            if c.name == column { c.data_type = new_type.clone(); }
        }
        Ok(())
    }

    /// ADR 0124 — DROP DEFAULT. No-op if none set.
    pub fn drop_column_default(&mut self, id: TableId, name: &str) -> Result<(), CatalogError> {
        let name = identifier(name)?;
        let table = self.tables.get(&id)
            .ok_or_else(|| CatalogError::NotFound(format!("table {}", id.0)))?;
        if !table.columns.iter().any(|c| c.name == name) {
            return Err(CatalogError::NotFound(format!("column {name}")));
        }
        self.tables.get_mut(&id).expect("validated table")
            .column_defaults.remove(&name);
        Ok(())
    }

    /// ADR 0157 — attach a named row/column/tenant policy
    /// to a table. `name` must be a valid identifier.
    /// Overwrites any existing policy with the same name.
    pub fn set_policy(
        &mut self,
        id: TableId,
        name: &str,
        policy: crate::access_policy::TableAccessPolicy,
    ) -> Result<(), CatalogError> {
        let name = identifier(name)?;
        let table = self.tables.get_mut(&id)
            .ok_or_else(|| CatalogError::NotFound(format!("table {}", id.0)))?;
        table.policies.insert(name, policy);
        Ok(())
    }

    /// ADR 0157 — drop a named policy. No-op if absent.
    pub fn drop_policy(&mut self, id: TableId, name: &str) -> Result<(), CatalogError> {
        let name = identifier(name)?;
        let table = self.tables.get_mut(&id)
            .ok_or_else(|| CatalogError::NotFound(format!("table {}", id.0)))?;
        table.policies.remove(&name);
        Ok(())
    }

    /// ADR 0157 — get a named policy.
    pub fn get_policy(&self, id: TableId, name: &str)
        -> Option<&crate::access_policy::TableAccessPolicy>
    {
        let name = identifier(name).ok()?;
        self.tables.get(&id)?.policies.get(&name)
    }

    /// ADR 0157 — iterate every policy attached to a table.
    pub fn policies_for_table(&self, id: TableId)
        -> Option<&BTreeMap<String, crate::access_policy::TableAccessPolicy>>
    {
        self.tables.get(&id).map(|t| &t.policies)
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
