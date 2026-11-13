//! G23 typed adapter for the independent G22 durable-history domain.
//! Every pinned catalog schema is stored under a versioned logical metadata key.
//! Existing G15/G21 domain directories are NEVER implicitly rewritten.

use std::collections::BTreeMap;
use std::path::Path;
use serde::{Deserialize, Serialize};

use crate::{Table, TableId};
use crate::epoch_row::{DecodedEpochRow, EpochRowError, EpochSchemas};
use crate::persistent_catalog::{PersistentCatalog, PersistentCatalogError};
use crate::schema_activation::{self, ActivationCommit, ActivationIntent, ActivationStage,
    PendingActivation, SchemaActivationReceipt, SchemaChanges, SchemaTimeline, LegacyDdlRetirement, LegacyDdlRetirementReceipt};
use crate::durable_mvcc::{DurableMvccError, DurableMvccStats, DurableMvccStore, DurableMvccTransaction};
use crate::typed_table::{
    encode_key, encode_row, validate_table, TableError, Value,
};
use crate::transaction_domain::DomainError;

const SCHEMA_PREFIX: &[u8] = b"\0bradian-domain-table-v1:";
const ROW_PREFIX: &[u8] = b"BRPK";
const PROPOSAL_PREFIX: &[u8] = b"\0bradian-schema-proposal-v1:";
const ABORT_PREFIX: &[u8] = b"\0bradian-schema-abort-v1:";
const COORD_PREFIX: &[u8] = b"\0bradian-ddl-coordinate-v1:";
const READY_PREFIX: &[u8] = b"\0bradian-ddl-readiness-v1:";

const LEGACY_DDL_PREFIXES: [&[u8]; 4] = [PROPOSAL_PREFIX, ABORT_PREFIX, COORD_PREFIX, READY_PREFIX];

/// Inactive readiness certificate. It fences the exact G30 coordination
/// decision; it is NOT an active schema epoch or a catalog DDL event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DdlReadinessRecord {
    pub table_id: TableId,
    pub coordination_lsn: u64,
    pub proposal_lsn: u64,
    pub catalog_lsn: u64,
    pub proposal_crc32: u32,
}

/// Inactive cross-store DDL fence. Catalog is not mutated; this binds one
/// G28 prepared proposal to the exact G11 catalog WAL horizon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DdlCoordinationRecord {
    pub table_id: TableId,
    pub proposal_lsn: u64,
    pub catalog_lsn: u64,
    pub proposal_crc32: u32,
}
const INLINE_CAPACITY: usize = 3545;
const ENCODING_KEY: &[u8] = b"\0bradian-row-encoding-v1";
const EPOCH_ENCODING: &[u8] = b"BRADIAN-EPOCH-ROWS-V1";

pub(crate) fn is_legacy_ddl_key(key: &[u8]) -> bool {
    key.starts_with(PROPOSAL_PREFIX) || key.starts_with(ABORT_PREFIX)
        || key.starts_with(COORD_PREFIX) || key.starts_with(READY_PREFIX)
}

fn is_metadata_key(key: &[u8]) -> bool {
    key == schema_activation::retirement_key() || schema_activation::is_activation_key(key) || key == ENCODING_KEY || key.starts_with(SCHEMA_PREFIX)
        || key.starts_with(PROPOSAL_PREFIX) || key.starts_with(ABORT_PREFIX)
        || key.starts_with(COORD_PREFIX) || key.starts_with(READY_PREFIX)
}

fn decode_source_row(
    definition: &Definition, epochs: &EpochSchemas, bytes: &[u8], enabled: bool, original_epoch: u64,
) -> Result<DecodedEpochRow, DomainError> {
    let decoded = if bytes.starts_with(b"BRVE") {
        if !enabled {
            return Err(DomainError::Corruption("versioned row predates epoch encoding activation".into()));
        }
        epochs.decode(bytes)
    } else { epochs.decode_legacy(definition.table.id, original_epoch, bytes) }
        .map_err(|err| DomainError::Corruption(err.to_string()))?;
    if decoded.table_id != definition.table.id {
        return Err(DomainError::Corruption("row envelope belongs to a different table".into()));
    }
    Ok(decoded)
}

fn project_decoded(
    definition: &Definition, epochs: &EpochSchemas, decoded: DecodedEpochRow,
) -> Result<Vec<Value>, DomainError> {
    let source = epochs.schema(decoded.table_id, decoded.epoch)
        .map_err(|err| DomainError::Corruption(err.to_string()))?;
    if source == &definition.table { return Ok(decoded.values); }
    let plan = crate::schema_evolution::plan_nullable_evolution(source, &definition.table)?;
    Ok(crate::schema_evolution::project_row(&plan, &decoded.values)?)
}

fn decode_physical_row(
    definition: &Definition, epochs: &EpochSchemas, bytes: &[u8], enabled: bool, original_epoch: u64,
) -> Result<Vec<Value>, DomainError> {
    project_decoded(definition, epochs,
        decode_source_row(definition, epochs, bytes, enabled, original_epoch)?)
}

fn map_catalog(err: PersistentCatalogError) -> DomainError {
    match err {
        PersistentCatalogError::Storage(err) => DomainError::Storage(err),
        other => DomainError::Corruption(other.to_string()),
    }
}

#[derive(Clone)]
struct Definition {
    table: Table,
    pk_positions: Vec<usize>,
}

fn table_id_from_row_key(key: &[u8]) -> Result<TableId, DomainError> {
    if key.len() < 13 || &key[..4] != ROW_PREFIX || key[4] != 1 {
        return Err(DomainError::Corruption(
            "MVCC domain contains unrecognized row namespace/version".into(),
        ));
    }
    Ok(TableId(u64::from_le_bytes(key[5..13].try_into().unwrap())))
}

fn key_from_row(def: &Definition, row: &[Value]) -> Result<Vec<u8>, DomainError> {
    if row.len() != def.table.columns.len() {
        return Err(DomainError::Table(TableError::Value(
            "row width does not match pinned schema".into(),
        )));
    }
    let pk = def.pk_positions.iter().map(|index| row[*index].clone()).collect::<Vec<_>>();
    Ok(encode_key(&def.table, &def.pk_positions, &pk)?)
}

/// A validated, exclusively owned view of a G22 durable-history WAL.
pub struct DurableTypedStore {
    store: DurableMvccStore,
    tables: BTreeMap<TableId, Definition>,
    schema_epochs: BTreeMap<TableId, u64>,
    epoch_schemas: EpochSchemas,
    original_epochs: BTreeMap<TableId, u64>,
    schema_timeline: SchemaTimeline,
    pending_activation: Option<PendingActivation>,
    catalog_horizon: Option<u64>,
    legacy_retirement: Option<(LegacyDdlRetirement, u64)>,
    proposals: BTreeMap<TableId, (Table, u64)>,
    aborted: BTreeMap<TableId, u64>,
    coordination: BTreeMap<TableId, (DdlCoordinationRecord, u64)>,
    readiness: BTreeMap<TableId, (DdlReadinessRecord, u64)>,
}

impl DurableTypedStore {
    /// Requires an explicitly provisioned G23 history domain; does not create metadata pins.
    /// All catalog tables need not be active. Replayed current definitions must
    /// match the supplied catalog; original pins and earlier epochs stay immutable.
    /// Unfinished activation requires the coordinated owner instead.
    pub fn open(
        domain_root: impl AsRef<Path>,
        catalog_tables: &BTreeMap<TableId, Table>,
    ) -> Result<Self, DomainError> {
        let store = DurableMvccStore::open(domain_root).map_err(map_mvcc)?;
        Self::from_store(store, Some(catalog_tables))
    }

    /// Recovery owns both locks and validates history before changing the catalog.
    pub(crate) fn open_coordinated(
        domain_root: impl AsRef<Path>, catalog: &PersistentCatalog,
    ) -> Result<Self, DomainError> {
        let store = DurableMvccStore::open(domain_root).map_err(map_mvcc)?;
        let mut owner = Self::from_store(store, None)?;
        if owner.pending_activation.is_some() {
            owner.finish_activation(catalog, &mut |_| Ok(()))?;
        }
        let tables = catalog.snapshot().tables().map(|t| (t.id, t.clone())).collect();
        owner.verify_catalog(&tables)?;
        owner.verify_ddl_catalog_horizon(catalog.last_lsn())?;
        Ok(owner)
    }

    fn from_store(
        store: DurableMvccStore, catalog_tables: Option<&BTreeMap<TableId, Table>>,
    ) -> Result<Self, DomainError> {
        let mut snapshot = store.begin();
        let records = snapshot.scan();
        snapshot.rollback();
        let mut tables = BTreeMap::new();
        for (key, bytes) in &records {
            if !key.starts_with(SCHEMA_PREFIX) { continue; }
            if key.len() != SCHEMA_PREFIX.len() + 8 {
                return Err(DomainError::Corruption("malformed shared-domain schema key".into()));
            }
            let id = TableId(u64::from_be_bytes(
                key[SCHEMA_PREFIX.len()..].try_into().unwrap()
            ));
            let table: Table = serde_json::from_slice(bytes)
                .map_err(|err| DomainError::Corruption(format!(
                    "malformed schema pin {}: {err}", id.0
                )))?;
            if table.id != id {
                return Err(DomainError::Corruption(format!(
                    "pinned schema {} differs from durable catalog", id.0
                )));
            }
            let pk_positions = validate_table(&table)?;
            if tables.insert(id, Definition { table, pk_positions }).is_some() {
                return Err(DomainError::Corruption("duplicate schema pin".into()));
            }
        }
        if tables.is_empty() {
            return Err(DomainError::Corruption(
                "shared MVCC domain has no registered catalog table schemas".into(),
            ));
        }
        // Validate EVERY retained version, not just the latest visible rows.
        // A malformed older payload or a tombstoned unknown-table key must
        // not become reachable through AS OF after the database is opened.
        let versions = store.retained_versions();
        let mut registered_at = BTreeMap::new();
        for (key, lsn, value) in &versions {
            if !key.starts_with(SCHEMA_PREFIX) { continue; }
            if key.len() != SCHEMA_PREFIX.len() + 8 {
                return Err(DomainError::Corruption("malformed historical schema key".into()));
            }
            let id = TableId(u64::from_be_bytes(
                key[SCHEMA_PREFIX.len()..].try_into().unwrap()
            ));
            let def = tables.get(&id).ok_or_else(|| DomainError::Corruption(
                format!("historical schema pin for unregistered table {}", id.0)
            ))?;
            let bytes = value.as_ref().ok_or_else(|| DomainError::Corruption(
                format!("historical schema pin {} was deleted", id.0)
            ))?;
            if serde_json::from_slice::<Table>(bytes).ok().as_ref() != Some(&def.table) {
                return Err(DomainError::Corruption(format!(
                    "historical schema pin {} differs from authoritative catalog", id.0
                )));
            }
            if registered_at.insert(id, *lsn).is_some() {
                return Err(DomainError::Corruption(format!(
                    "historical schema pin {} was rewritten", id.0
                )));
            }
        }
        let mut epoch_schemas = EpochSchemas::default();
        for (id, epoch) in &registered_at {
            epoch_schemas.register(*epoch, tables[id].table.clone())
                .map_err(|err| DomainError::Corruption(err.to_string()))?;
        }
        let mut encoding_lsn = None;
        for (key, lsn, value) in &versions {
            if key.as_slice() != ENCODING_KEY { continue; }
            if value.as_deref() != Some(EPOCH_ENCODING)
                || registered_at.values().any(|epoch| *lsn <= *epoch)
                || encoding_lsn.replace(*lsn).is_some()
            {
                return Err(DomainError::Corruption("epoch row encoding decision was deleted, rewritten or is invalid".into()));
            }
        }
        let mut proposals = BTreeMap::new();
        for (key, lsn, value) in &versions {
            if !key.starts_with(PROPOSAL_PREFIX) { continue; }
            if key.len() != PROPOSAL_PREFIX.len() + 8 {
                return Err(DomainError::Corruption("invalid G28 proposal key".into()));
            }
            let id = TableId(u64::from_be_bytes(
                key[PROPOSAL_PREFIX.len()..].try_into().unwrap()
            ));
            let old = tables.get(&id).ok_or_else(|| DomainError::Corruption(
                "G28 proposal references an unregistered table".into()
            ))?;
            if registered_at.get(&id).is_none_or(|registration| *lsn <= *registration) {
                return Err(DomainError::Corruption("G28 proposal predates table registration".into()));
            }
            let data = value.as_ref().ok_or_else(|| DomainError::Corruption(
                "G28 prepared proposal was deleted".into()
            ))?;
            let proposed: Table = serde_json::from_slice(data).map_err(|e|
                DomainError::Corruption(format!("invalid G28 proposed schema: {e}"))
            )?;
            crate::schema_evolution::plan_nullable_additions(&old.table, &proposed)?;
            if proposals.insert(id, (proposed, *lsn)).is_some() {
                return Err(DomainError::Corruption("G28 proposal was rewritten".into()));
            }
        }
        let mut aborted = BTreeMap::new();
        for (key, lsn, value) in &versions {
            if !key.starts_with(ABORT_PREFIX) { continue; }
            if key.len() != ABORT_PREFIX.len() + 8 {
                return Err(DomainError::Corruption("invalid G29 abort marker key".into()));
            }
            let id = TableId(u64::from_be_bytes(
                key[ABORT_PREFIX.len()..].try_into().unwrap()
            ));
            let (_, prepared_at) = proposals.get(&id).ok_or_else(||
                DomainError::Corruption("G29 abort marker without prepared intent".into())
            )?;
            if *lsn <= *prepared_at || value.as_deref() != Some(b"ABORTED-V1".as_slice()) {
                return Err(DomainError::Corruption("invalid G29 aborted intent sequence or payload".into()));
            }
            if aborted.insert(id, *lsn).is_some() {
                return Err(DomainError::Corruption("G29 abort marker was rewritten".into()));
            }
        }
        let mut coordination = BTreeMap::new();
        for (key, lsn, value) in &versions {
            if !key.starts_with(COORD_PREFIX) { continue; }
            if key.len() != COORD_PREFIX.len() + 8 {
                return Err(DomainError::Corruption("malformed G30 DDL coordination key".into()));
            }
            let id = TableId(u64::from_be_bytes(
                key[COORD_PREFIX.len()..].try_into().unwrap()
            ));
            let data = value.as_ref().ok_or_else(|| DomainError::Corruption(
                "G30 coordination record cannot be deleted".into()
            ))?;
            let record: DdlCoordinationRecord = serde_json::from_slice(data)
                .map_err(|e| DomainError::Corruption(format!(
                    "malformed G30 coordination payload: {e}"
                )))?;
            let (proposed, proposal_lsn) = proposals.get(&id).ok_or_else(||
                DomainError::Corruption("G30 coordination has no prepared proposal".into())
            )?;
            if record.table_id != id || record.proposal_lsn != *proposal_lsn
                || record.proposal_crc32 != crc32fast::hash(&serde_json::to_vec(proposed)?)
                || *lsn <= *proposal_lsn
                || aborted.get(&id).is_some_and(|aborted_at| *aborted_at <= *lsn)
            {
                return Err(DomainError::Corruption(
                    "G30 coordination does not match its proposal or chronology".into()
                ));
            }
            if coordination.insert(id, (record, *lsn)).is_some() {
                return Err(DomainError::Corruption(
                    "G30 coordination record was rewritten".into()
                ));
            }
        }
        let mut readiness = BTreeMap::new();
        for (key, lsn, value) in &versions {
            if !key.starts_with(READY_PREFIX) { continue; }
            if key.len() != READY_PREFIX.len() + 8 {
                return Err(DomainError::Corruption("G31 invalid readiness key".into()));
            }
            let id = TableId(u64::from_be_bytes(
                key[READY_PREFIX.len()..].try_into().unwrap()
            ));
            let bytes = value.as_ref().ok_or_else(||
                DomainError::Corruption("G31 readiness cannot be deleted".into())
            )?;
            let record: DdlReadinessRecord = serde_json::from_slice(bytes).map_err(|e|
                DomainError::Corruption(format!("G31 invalid readiness payload: {e}"))
            )?;
            let (fence, coordinated_at) = coordination.get(&id).ok_or_else(||
                DomainError::Corruption("G31 readiness without G30 coordination".into())
            )?;
            if record.table_id != id
                || record.coordination_lsn != *coordinated_at
                || record.proposal_lsn != fence.proposal_lsn
                || record.catalog_lsn != fence.catalog_lsn
                || record.proposal_crc32 != fence.proposal_crc32
                || *lsn <= *coordinated_at
                || aborted.get(&id).is_some_and(|aborted_at| *aborted_at <= *lsn)
            {
                return Err(DomainError::Corruption(
                    "G31 readiness has incompatible fence, timeline or cancellation".into()
                ));
            }
            if readiness.insert(id, (record, *lsn)).is_some() {
                return Err(DomainError::Corruption("G31 readiness was rewritten".into()));
            }
        }
        let timeline = tables.iter().map(|(id, def)|
            (*id, BTreeMap::from([(registered_at[id], def.table.clone())]))).collect();
        let legacy_retirement = schema_activation::load_retirement(&versions)?;
        if let Some((record, retired_at)) = &legacy_retirement {
            if versions.iter().any(|(key, lsn, _)| schema_activation::is_activation_key(key) && lsn <= retired_at) {
                return Err(DomainError::Corruption("legacy retirement must precede coordinated activation".into()));
            }
            if coordination.values().any(|(fence, _)| fence.catalog_lsn != record.catalog_lsn) {
                return Err(DomainError::Corruption("retired legacy catalog horizon differs".into()));
            }
        }
        let changes = SchemaChanges::load(&versions, timeline, encoding_lsn, store.latest_lsn(),
            legacy_retirement.is_none() && versions.iter().any(|(key, _, _)| is_legacy_ddl_key(key)),
            legacy_retirement.as_ref().map(|(record, _)| record.catalog_lsn))?;
        let mut current_epochs = BTreeMap::new();
        for (id, schemas) in &changes.timeline {
            for (epoch, table) in schemas {
                epoch_schemas.register(*epoch, table.clone())
                    .map_err(|err| DomainError::Corruption(err.to_string()))?;
            }
            let (epoch, table) = schemas.last_key_value().expect("registered timeline");
            current_epochs.insert(*id, *epoch);
            tables.insert(*id, Definition { table: table.clone(), pk_positions: validate_table(table)? });
        }
        for (key, lsn, value) in &versions {
            if is_metadata_key(key) { continue; }
            let id = table_id_from_row_key(key)?;
            let def = tables.get(&id).ok_or_else(|| DomainError::Corruption(
                format!("historical row refers to unregistered table {}", id.0)
            ))?;
            if registered_at.get(&id).is_none_or(|registered| lsn < registered) {
                return Err(DomainError::Corruption(format!(
                    "historical row predates schema registration for table {}", id.0
                )));
            }
            if let Some(bytes) = value {
                let decoded = decode_source_row(def, &epoch_schemas, bytes,
                    encoding_lsn.is_some_and(|activated| *lsn > activated), registered_at[&id])?;
                let active_epoch = changes.timeline[&id].range(..=*lsn).next_back()
                    .ok_or_else(|| DomainError::Corruption("row predates its schema timeline".into()))?.0;
                if decoded.epoch != *active_epoch {
                    return Err(DomainError::Corruption("row epoch differs from schema active at commit".into()));
                }
                let row = project_decoded(def, &epoch_schemas, decoded)?;
                if key_from_row(def, &row)? != *key {
                    return Err(DomainError::Corruption(
                        "historical row key differs from decoded primary key".into()
                    ));
                }
            }
        }
        let owner = Self { store, tables, schema_epochs: current_epochs, epoch_schemas,
            original_epochs: registered_at, schema_timeline: changes.timeline,
            pending_activation: changes.pending, catalog_horizon: changes.catalog_horizon, legacy_retirement,
            proposals, aborted, coordination, readiness };
        if let Some(expected) = catalog_tables {
            if owner.pending_activation.is_some() {
                return Err(DomainError::Corruption("unfinished schema activation requires coordinated open".into()));
            }
            owner.verify_catalog(expected)?;
        }
        Ok(owner)
    }

    pub fn active_tables(&self) -> Vec<TableId> {
        self.tables.keys().copied().collect()
    }

    pub fn table(&self, id: TableId) -> Option<&Table> {
        self.tables.get(&id).map(|definition| &definition.table)
    }

    /// Return the immutable schema active at a retained historical horizon,
    /// with its original registration or G36 activation WAL position.
    pub fn schema_as_of(&self, id: TableId, lsn: u64) -> Result<(Table, u64), DomainError> {
        self.check_schema_as_of(id, lsn)?;
        self.schema_timeline.get(&id).and_then(|schemas| schemas.range(..=lsn).next_back())
            .map(|(epoch, table)| (table.clone(), *epoch))
            .ok_or_else(|| DomainError::Corruption("schema not active at requested horizon".into()))
    }

    /// The registered schema epochs are extracted from verified G22 history,
    /// not inferred from the current catalog or process start time.
    pub fn schema_epochs(&self) -> Vec<(TableId, u64)> {
        self.schema_timeline.iter().flat_map(|(id, schemas)| schemas.keys().map(|lsn| (*id, *lsn))).collect()
    }

    fn require_legacy_ddl(&self) -> Result<(), DomainError> {
        if self.catalog_horizon.is_some() || self.pending_activation.is_some() {
            return Err(DomainError::Corruption("legacy DDL intents cannot follow coordinated schema activation".into()));
        }
        Ok(())
    }

    /// G28 prepares one immutable schema proposal in the SAME G22 WAL as row
    /// versions. This is a durable intent only; it cannot activate a new
    /// schema, alter the G11 catalog or change row decoding.
    pub fn prepare_nullable_addition(&self, id: TableId, proposed: &Table) -> Result<u64, DomainError> {
        self.require_legacy_ddl()?;
        let old = self.tables.get(&id).ok_or(DomainError::UnknownTable(id.0))?;
        crate::schema_evolution::plan_nullable_additions(&old.table, proposed)?;
        let mut key = PROPOSAL_PREFIX.to_vec();
        key.extend_from_slice(&id.0.to_be_bytes());
        let bytes = serde_json::to_vec(proposed)?;
        let mut tx = self.store.begin();
        // Re-check the physical namespace using the transaction snapshot,
        // not just this handle's cache; duplicate proposals fail closed.
        tx.insert(&key, &bytes).map_err(map_mvcc)?;
        tx.commit().map_err(map_mvcc)
    }

    /// List recovered G28 prepared intents, without treating any proposal
    /// as an active schema revision.
    pub fn prepared_schema_proposals(&self) -> Vec<(TableId, Table, u64)> {
        if self.legacy_retirement.is_some() { return Vec::new(); }
        self.proposals.iter().map(|(id, (table, lsn))|
            (*id, table.clone(), *lsn)).collect()
    }

    /// G29: durably retire a prepared proposal without changing the G11
    /// catalog or any historical row. The abort marker is append-only and
    /// replay-validated. It is deliberately not an ALTER/activation protocol.
    pub fn abort_prepared_schema(&self, id: TableId) -> Result<u64, DomainError> {
        self.require_legacy_ddl()?;
        let mut proposal_key = PROPOSAL_PREFIX.to_vec();
        proposal_key.extend_from_slice(&id.0.to_be_bytes());
        let mut abort_key = ABORT_PREFIX.to_vec();
        abort_key.extend_from_slice(&id.0.to_be_bytes());
        let mut tx = self.store.begin();
        // Use the current MVCC snapshot, not the opening cache: proposals
        // added after this owner was opened remain discoverable.
        let proposed = tx.get(&proposal_key).map_err(map_mvcc)?
            .ok_or_else(|| DomainError::Corruption(
                "cannot abort a missing prepared schema proposal".into()
            ))?;
        let table: Table = serde_json::from_slice(&proposed)?;
        let original = self.tables.get(&id).ok_or(DomainError::UnknownTable(id.0))?;
        crate::schema_evolution::plan_nullable_additions(&original.table, &table)?;
        if tx.get(&abort_key).map_err(map_mvcc)?.is_some() {
            return Err(DomainError::Corruption("schema proposal was already aborted".into()));
        }
        tx.insert(&abort_key, b"ABORTED-V1").map_err(map_mvcc)?;
        tx.commit().map_err(map_mvcc)
    }

    /// Recovery status is a snapshot of the history WAL at engine open.
    /// Reopen the owner to inspect new decisions made during this process.
    pub fn aborted_schema_proposals(&self) -> Vec<(TableId, u64)> {
        self.aborted.iter().map(|(id, lsn)| (*id, *lsn)).collect()
    }

    /// G30: record the exact G11 catalog WAL horizon alongside the durable
    /// proposal. No ALTER is issued; a later catalog modification becomes a
    /// hard reopen failure until a separately qualified recovery protocol exists.
    pub fn coordinate_prepared_schema(
        &self, id: TableId, catalog_lsn: u64,
    ) -> Result<u64, DomainError> {
        self.require_legacy_ddl()?;
        let mut proposal_key = PROPOSAL_PREFIX.to_vec();
        proposal_key.extend_from_slice(&id.0.to_be_bytes());
        let mut abort_key = ABORT_PREFIX.to_vec();
        abort_key.extend_from_slice(&id.0.to_be_bytes());
        let mut coordinate_key = COORD_PREFIX.to_vec();
        coordinate_key.extend_from_slice(&id.0.to_be_bytes());
        let versions = self.store.retained_versions();
        let proposal_lsn = versions.iter().find(|(key, _, value)|
            key == &proposal_key && value.is_some()
        ).map(|(_, lsn, _)| *lsn).ok_or_else(||
            DomainError::Corruption("G30 requires a durable prepared proposal".into())
        )?;
        let mut tx = self.store.begin();
        let bytes = tx.get(&proposal_key).map_err(map_mvcc)?.ok_or_else(||
            DomainError::Corruption("G30 prepared proposal is absent".into())
        )?;
        if tx.get(&abort_key).map_err(map_mvcc)?.is_some() {
            return Err(DomainError::Corruption("G30 proposal already aborted".into()));
        }
        let proposed: Table = serde_json::from_slice(&bytes)?;
        let original = self.tables.get(&id).ok_or(DomainError::UnknownTable(id.0))?;
        crate::schema_evolution::plan_nullable_additions(&original.table, &proposed)?;
        let record = DdlCoordinationRecord {
            table_id: id,
            proposal_lsn,
            catalog_lsn,
            proposal_crc32: crc32fast::hash(&serde_json::to_vec(&proposed)?),
        };
        tx.insert(&coordinate_key, &serde_json::to_vec(&record)?).map_err(map_mvcc)?;
        tx.commit().map_err(map_mvcc)
    }

    /// Recovered G30 fences at engine open; no schema activation implied.
    pub fn ddl_coordination_records(&self) -> Vec<(DdlCoordinationRecord, u64)> {
        self.coordination.values().cloned().collect()
    }

    /// Detect independent catalog mutations after an inactive coordination
    /// fence was committed. Fail closed; never silently complete ALTER.
    pub fn verify_ddl_catalog_horizon(&self, catalog_lsn: u64) -> Result<(), DomainError> {
        if self.catalog_horizon.is_some_and(|expected| expected != catalog_lsn) {
            return Err(DomainError::Corruption("activated schema catalog horizon differs".into()));
        }
        if self.legacy_retirement.is_some() { return Ok(()); }
        for (record, _) in self.coordination.values() {
            if record.catalog_lsn != catalog_lsn {
                return Err(DomainError::Corruption(format!(
                    "G30 DDL coordination catalog LSN mismatch for table {}",
                    record.table_id.0
                )));
            }
        }
        Ok(())
    }

    /// G31: record an immutable readiness decision in the history WAL.
    /// No G11 catalog update or row encoding change occurs here.
    pub fn certify_schema_readiness(
        &self, id: TableId, catalog_lsn: u64,
    ) -> Result<u64, DomainError> {
        self.require_legacy_ddl()?;
        let mut coordinate_key = COORD_PREFIX.to_vec();
        coordinate_key.extend_from_slice(&id.0.to_be_bytes());
        let mut abort_key = ABORT_PREFIX.to_vec();
        abort_key.extend_from_slice(&id.0.to_be_bytes());
        let mut ready_key = READY_PREFIX.to_vec();
        ready_key.extend_from_slice(&id.0.to_be_bytes());
        let coordinated_at = self.store.retained_versions().into_iter()
            .find(|(key, _, value)| *key == coordinate_key && value.is_some())
            .map(|(_, lsn, _)| lsn).ok_or_else(||
                DomainError::Corruption("G31 requires a durable G30 coordination fence".into())
            )?;
        let mut tx = self.store.begin();
        let bytes = tx.get(&coordinate_key).map_err(map_mvcc)?.ok_or_else(||
            DomainError::Corruption("G31 coordination fence absent".into())
        )?;
        let fence: DdlCoordinationRecord = serde_json::from_slice(&bytes)?;
        if fence.table_id != id || fence.catalog_lsn != catalog_lsn {
            return Err(DomainError::Corruption("G31 catalog LSN or table mismatch".into()));
        }
        if tx.get(&abort_key).map_err(map_mvcc)?.is_some() {
            return Err(DomainError::Corruption("G31 proposal was cancelled".into()));
        }
        let record = DdlReadinessRecord {
            table_id: id,
            coordination_lsn: coordinated_at,
            proposal_lsn: fence.proposal_lsn,
            catalog_lsn: fence.catalog_lsn,
            proposal_crc32: fence.proposal_crc32,
        };
        tx.insert(&ready_key, &serde_json::to_vec(&record)?).map_err(map_mvcc)?;
        tx.commit().map_err(map_mvcc)
    }

    /// Recovered G31 readiness certificates at engine open; a certificate
    /// is NEVER equivalent to activated schema DDL.
    pub fn ddl_readiness_records(&self) -> Vec<(DdlReadinessRecord, u64)> {
        self.readiness.values().cloned().collect()
    }

    /// Commit a one-way encoding capability decision. Existing rows stay unchanged.
    /// Readers from before G35 reject the new metadata namespace on reopen.
    pub fn enable_epoch_row_writes(&self) -> Result<u64, DomainError> {
        let mut tx = self.store.begin();
        tx.insert(ENCODING_KEY, EPOCH_ENCODING).map_err(map_mvcc)?;
        tx.commit().map_err(map_mvcc)
    }

    /// Status at the current committed horizon, including a transition made
    /// after this handle opened. The decision is not an active schema revision.
    pub fn epoch_row_writes_enabled(&self) -> Result<bool, DomainError> {
        self.encoding_enabled_at(self.last_lsn())
    }

    fn encoding_enabled_at(&self, lsn: u64) -> Result<bool, DomainError> {
        match self.store.read_as_of(ENCODING_KEY, lsn).map_err(map_mvcc)? {
            None => Ok(false),
            Some(value) if value == EPOCH_ENCODING => Ok(true),
            Some(_) => Err(DomainError::Corruption("unsupported epoch row encoding decision".into())),
        }
    }

    pub(crate) fn retire_legacy_schema_intents(
        &mut self, catalog: &PersistentCatalog,
    ) -> Result<LegacyDdlRetirementReceipt, DomainError> {
        if self.legacy_retirement.is_some() || self.catalog_horizon.is_some()
            || self.pending_activation.is_some() || self.stats().active_snapshots != 0
            || !catalog.proposed_table_revisions().is_empty()
        { return Err(DomainError::Corruption("legacy retirement requires an unretired quiescent root".into())); }
        self.verify_ddl_catalog_horizon(catalog.last_lsn())?;
        let expected = catalog.snapshot().tables().map(|t| (t.id, t.clone())).collect();
        self.verify_catalog(&expected)?;
        let (records, crc32) = schema_activation::retirement_digest(&self.store.retained_versions_for_prefixes(&LEGACY_DDL_PREFIXES))?;
        if records == 0 { return Err(DomainError::Corruption("no legacy DDL intents to retire".into())); }
        let record = LegacyDdlRetirement { version: 1, catalog_lsn: catalog.last_lsn(), records, crc32 };
        let mut tx = self.store.begin();
        tx.insert(schema_activation::retirement_key(), &serde_json::to_vec(&record)?).map_err(map_mvcc)?;
        let lsn = tx.commit().map_err(map_mvcc)?;
        let result = LegacyDdlRetirementReceipt { history_lsn: lsn, catalog_lsn: record.catalog_lsn, retired_records: records };
        self.catalog_horizon = Some(record.catalog_lsn);
        self.legacy_retirement = Some((record, lsn));
        Ok(result)
    }

    pub fn legacy_schema_intents_retired(&self) -> bool { self.legacy_retirement.is_some() }

    fn verify_catalog(&self, expected: &BTreeMap<TableId, Table>) -> Result<(), DomainError> {
        for (id, def) in &self.tables {
            if expected.get(id) != Some(&def.table) {
                return Err(DomainError::SchemaMismatch(id.0));
            }
        }
        Ok(())
    }

    pub(crate) fn plan_schema_activation(
        &self, catalog: &PersistentCatalog, proposed: Table,
    ) -> Result<PendingActivation, DomainError> {
        if !self.epoch_row_writes_enabled()? || self.stats().active_snapshots != 0
            || self.pending_activation.is_some() || !catalog.proposed_table_revisions().is_empty()
        {
            return Err(DomainError::Corruption("ALTER requires epoch encoding, no active snapshots and no pending proposals".into()));
        }
        self.verify_ddl_catalog_horizon(catalog.last_lsn())?;
        if self.legacy_retirement.is_none() && !self.store.retained_versions_for_prefixes(&LEGACY_DDL_PREFIXES).is_empty()
        {
            return Err(DomainError::Corruption("legacy DDL metadata requires a separate conversion before ALTER".into()));
        }
        let old = self.tables.get(&proposed.id).ok_or(DomainError::UnknownTable(proposed.id.0))?;
        schema_activation::validate_nullable_evolution(&old.table, &proposed)?;
        let expected = catalog.snapshot().tables().map(|t| (t.id, t.clone())).collect();
        self.verify_catalog(&expected)?;
        // Reserve space for G11's event/receipt wrappers around the same definition.
        if serde_json::to_vec(&proposed)?.len() + 256 > INLINE_CAPACITY {
            return Err(DomainError::Corruption("schema revision exceeds bounded journal capacity".into()));
        }
        let pending = PendingActivation {
            intent_lsn: self.last_lsn().checked_add(1)
                .ok_or_else(|| DomainError::Corruption("history sequence exhausted".into()))?,
            intent: ActivationIntent { version: 1,
                previous_epoch: self.schema_epochs[&proposed.id],
                catalog_before_lsn: catalog.last_lsn(), proposed },
        };
        pending.catalog_lsn()?;
        Ok(pending)
    }

    pub(crate) fn publish_schema_activation(
        &mut self, catalog: &PersistentCatalog, pending: PendingActivation,
        hook: &mut impl FnMut(ActivationStage) -> Result<(), DomainError>,
    ) -> Result<SchemaActivationReceipt, DomainError> {
        if self.last_lsn().checked_add(1) != Some(pending.intent_lsn)
            || self.pending_activation.is_some()
            || catalog.last_lsn() != pending.intent.catalog_before_lsn
        { return Err(DomainError::Conflict); }
        let mut tx = self.store.begin();
        tx.insert(&schema_activation::intent_key(pending.intent_lsn),
            &serde_json::to_vec(&pending.intent)?).map_err(map_mvcc)?;
        let committed = tx.commit().map_err(map_mvcc)?;
        if committed != pending.intent_lsn {
            return Err(DomainError::Corruption("schema intent WAL position differs".into()));
        }
        self.pending_activation = Some(pending);
        hook(ActivationStage::IntentSynced)?;
        self.finish_activation(catalog, hook)
    }

    fn finish_activation(
        &mut self, catalog: &PersistentCatalog,
        hook: &mut impl FnMut(ActivationStage) -> Result<(), DomainError>,
    ) -> Result<SchemaActivationReceipt, DomainError> {
        let pending = self.pending_activation.as_ref().ok_or_else(||
            DomainError::Corruption("activation has no durable intent".into()))?.clone();
        if self.last_lsn() != pending.intent_lsn {
            return Err(DomainError::Corruption("history changed after schema intent".into()));
        }
        let proposed = &pending.intent.proposed;
        let base = pending.intent.catalog_before_lsn;
        let final_lsn = pending.catalog_lsn()?;
        let actual = catalog.last_lsn();
        let proposals = catalog.proposed_table_revisions();
        let snapshot = catalog.snapshot();
        let intermediate = catalog.pending_table_revision(proposed.id);
        let state_valid = if actual == base {
            proposals.is_empty()
        } else if actual == base + 1 {
            proposals.len() == 1 && intermediate.as_ref().is_some_and(|r|
                r.proposal_lsn == base + 1 && &r.proposed == proposed)
        } else if actual == final_lsn { proposals.is_empty() } else { false };
        if !state_valid {
            return Err(DomainError::Corruption("catalog diverged from the durable activation decision".into()));
        }
        for (id, def) in &self.tables {
            let expected = if *id == proposed.id && actual == final_lsn { proposed } else { &def.table };
            if snapshot.table(*id) != Some(expected) {
                return Err(DomainError::Corruption("catalog table differs during schema recovery".into()));
            }
        }
        if actual == base {
            let old = &self.tables[&proposed.id].table;
            let lsn = if crate::schema_evolution::plan_nullable_additions(old, proposed).is_ok() {
                catalog.propose_table_revision(proposed.id, proposed.clone())
            } else { catalog.propose_nullable_revision(proposed.id, proposed.clone()) }.map_err(map_catalog)?;
            if lsn != base + 1 { return Err(DomainError::Corruption("catalog proposal LSN differs".into())); }
        }
        hook(ActivationStage::CatalogProposed)?;
        if catalog.last_lsn() == base + 1 {
            let proposal = catalog.pending_table_revision(proposed.id).ok_or_else(||
                DomainError::Corruption("catalog proposal disappeared".into()))?;
            if catalog.activate_table_revision(proposal).map_err(map_catalog)? != final_lsn {
                return Err(DomainError::Corruption("catalog activation LSN differs".into()));
            }
        }
        hook(ActivationStage::CatalogActivated)?;
        let record = ActivationCommit { version: 1, intent_lsn: pending.intent_lsn, catalog_lsn: final_lsn };
        let mut tx = self.store.begin();
        tx.insert(&schema_activation::activation_key(pending.intent_lsn),
            &serde_json::to_vec(&record)?).map_err(map_mvcc)?;
        let epoch = tx.commit().map_err(map_mvcc)?;
        if pending.intent_lsn.checked_add(1) != Some(epoch) {
            return Err(DomainError::Corruption("schema activation WAL sequence differs".into()));
        }
        self.epoch_schemas.register(epoch, proposed.clone())
            .map_err(|err| DomainError::Corruption(err.to_string()))?;
        self.tables.insert(proposed.id, Definition { table: proposed.clone(), pk_positions: validate_table(proposed)? });
        self.schema_epochs.insert(proposed.id, epoch);
        self.schema_timeline.get_mut(&proposed.id).expect("existing table timeline").insert(epoch, proposed.clone());
        self.pending_activation = None;
        self.catalog_horizon = Some(final_lsn);
        hook(ActivationStage::HistoryActivated)?;
        Ok(SchemaActivationReceipt { table_id: proposed.id, intent_lsn: pending.intent_lsn,
            schema_epoch: epoch, catalog_lsn: final_lsn })
    }

    pub fn stats(&self) -> DurableMvccStats { self.store.stats() }
    pub fn retention_floor(&self) -> u64 { self.store.retention_floor() }

    pub fn last_lsn(&self) -> u64 { self.store.latest_lsn() }

    /// Capture the complete version-key store, including retained historical
    /// values, tombstones, schema pins and the synchronized retention floor.
    pub fn backup_to(&self, destination: impl AsRef<Path>) -> Result<crate::backup::BackupReceipt, DomainError> {
        self.store.backup_to(destination).map_err(map_mvcc)
    }

    pub(crate) fn backup_with_stats(
        &self, destination: impl AsRef<Path>,
    ) -> Result<(crate::backup::BackupReceipt, DurableMvccStats), DomainError> {
        self.store.backup_with_stats(destination).map_err(map_mvcc)
    }

    pub fn checkpoint(&self) -> Result<u64, DomainError> {
        self.store.checkpoint().map_err(map_mvcc)
    }

    /// Verify the original immutable pin existed at the requested horizon.
    /// G36 revisions are selected separately from the validated schema timeline.
    fn check_schema_as_of(&self, id: TableId, lsn: u64) -> Result<(), DomainError> {
        let original = self.epoch_schemas.schema(id, *self.original_epochs.get(&id)
            .ok_or(DomainError::UnknownTable(id.0))?)
            .map_err(|err| DomainError::Corruption(err.to_string()))?;
        let mut schema_key = SCHEMA_PREFIX.to_vec();
        schema_key.extend_from_slice(&id.0.to_be_bytes());
        let historical = self.store.read_as_of(&schema_key, lsn).map_err(map_mvcc)?;
        if historical.as_deref() != Some(serde_json::to_vec(original)
            .map_err(|e| DomainError::Corruption(e.to_string()))?.as_slice())
        {
            return Err(DomainError::Corruption(
                format!("table {} schema not active at historical WAL LSN {lsn}", id.0),
            ));
        }
        Ok(())
    }

    pub fn get_as_of(
        &self, id: TableId, pk: &[Value], lsn: u64,
    ) -> Result<Option<Vec<Value>>, DomainError> {
        let (table, _) = self.schema_as_of(id, lsn)?;
        let def = &Definition { pk_positions: validate_table(&table)?, table };
        let key = encode_key(&def.table, &def.pk_positions, pk)?;
        self.store.read_as_of(&key, lsn).map_err(map_mvcc)?
            .map(|bytes| {
                let row = decode_physical_row(def, &self.epoch_schemas, &bytes,
                    self.encoding_enabled_at(lsn)?, self.original_epochs[&id])?;
                if key_from_row(def, &row)? != key {
                    return Err(DomainError::Corruption("historical row primary key differs".into()));
                }
                Ok(row)
            }).transpose()
    }

    /// Read-only historical full-table scan. The G22 retention floor rejects
    /// queries before the earliest permitted WAL horizon.
    pub fn scan_as_of(&self, id: TableId, lsn: u64) -> Result<Vec<Vec<Value>>, DomainError> {
        let (table, _) = self.schema_as_of(id, lsn)?;
        let def = &Definition { pk_positions: validate_table(&table)?, table };
        let records = self.store.scan_as_of(lsn).map_err(map_mvcc)?;
        let encoding_enabled = self.encoding_enabled_at(lsn)?;
        let mut rows = Vec::new();
        for (key, bytes) in records {
            if is_metadata_key(&key) { continue; }
            let table_id = table_id_from_row_key(&key)?;
            if table_id != id { continue; }
            let row = decode_physical_row(def, &self.epoch_schemas, &bytes, encoding_enabled, self.original_epochs[&id])?;
            if key_from_row(def, &row)? != key {
                return Err(DomainError::Corruption("historical scan key differs".into()));
            }
            rows.push(row);
        }
        Ok(rows)
    }

    pub fn begin(&self) -> DurableTypedTransaction<'_> {
        DurableTypedTransaction { owner: self, inner: self.store.begin() }
    }
}

fn map_mvcc(err: DurableMvccError) -> DomainError {
    match err {
        DurableMvccError::Store(err) => DomainError::Storage(err),
        DurableMvccError::DuplicateKey => DomainError::Table(TableError::DuplicateKey),
        DurableMvccError::NotFound => DomainError::Table(TableError::NotFound),
        DurableMvccError::Conflict | DurableMvccError::ScanConflict => DomainError::Conflict,
        other => DomainError::Corruption(other.to_string()),
    }
}

/// Typed SQL-accessible transaction over G22 durable versions and G15 typed key encoding.
pub struct DurableTypedTransaction<'a> {
    owner: &'a DurableTypedStore,
    inner: DurableMvccTransaction<'a>,
}

impl DurableTypedTransaction<'_> {
    pub fn start_lsn(&self) -> u64 { self.inner.start_lsn() }

    fn definition(&self, id: TableId) -> Result<&Definition, DomainError> {
        self.owner.tables.get(&id).ok_or(DomainError::UnknownTable(id.0))
    }

    pub fn get(&self, id: TableId, key: &[Value]) -> Result<Option<Vec<Value>>, DomainError> {
        let def = self.definition(id)?;
        let physical_key = encode_key(&def.table, &def.pk_positions, key)?;
        self.inner.get(&physical_key).map_err(map_mvcc)?.map(|bytes| {
            let row = decode_physical_row(def, &self.owner.epoch_schemas, &bytes,
                self.epoch_encoding_enabled()?, self.owner.original_epochs[&id])?;
            if key_from_row(def, &row)? != physical_key {
                return Err(DomainError::Corruption(
                    "MVCC row does not match its physical primary key".into(),
                ));
            }
            Ok(row)
        }).transpose()
    }

    pub fn scan(&mut self, id: TableId) -> Result<Vec<Vec<Value>>, DomainError> {
        if !self.owner.tables.contains_key(&id) {
            return Err(DomainError::UnknownTable(id.0));
        }
        let records = self.inner.scan();
        let encoding_enabled = self.epoch_encoding_enabled()?;
        let mut rows = Vec::new();
        for (key, bytes) in records {
            // The full physical scan participates in G20's conservative
            // global LSN change validation when the transaction writes.
            if is_metadata_key(&key) { continue; }
            let table_id = table_id_from_row_key(&key)?;
            if table_id != id { continue; }
            let def = self.definition(id)?;
            let row = decode_physical_row(def, &self.owner.epoch_schemas, &bytes, encoding_enabled, self.owner.original_epochs[&id])?;
            if key_from_row(def, &row)? != key {
                return Err(DomainError::Corruption(
                    "MVCC scan found row with mismatched physical key".into(),
                ));
            }
            rows.push(row);
        }
        Ok(rows)
    }

    fn epoch_encoding_enabled(&self) -> Result<bool, DomainError> {
        match self.inner.get(ENCODING_KEY).map_err(map_mvcc)? {
            None => Ok(false),
            Some(value) if value == EPOCH_ENCODING => Ok(true),
            Some(_) => Err(DomainError::Corruption("unsupported epoch row encoding decision".into())),
        }
    }

    fn encode_physical_row(&self, def: &Definition, row: &[Value]) -> Result<Vec<u8>, DomainError> {
        let bytes = if self.epoch_encoding_enabled()? {
            self.owner.epoch_schemas.encode(def.table.id, self.owner.schema_epochs[&def.table.id], row)
                .map_err(|err| match err {
                    EpochRowError::Typed(err) => DomainError::Table(err),
                    other => DomainError::Table(TableError::Value(other.to_string())),
                })?
        } else { encode_row(&def.table, row)? };
        if bytes.len() > INLINE_CAPACITY {
            return Err(DomainError::Table(TableError::Value(
                "encoded row exceeds physical inline limit".into()
            )));
        }
        Ok(bytes)
    }

    pub fn insert(&mut self, id: TableId, row: &[Value]) -> Result<(), DomainError> {
        let def = self.definition(id)?;
        let bytes = self.encode_physical_row(def, row)?;
        let key = key_from_row(def, row)?;
        self.inner.insert(&key, &bytes).map_err(map_mvcc)
    }

    pub fn replace(&mut self, id: TableId, row: &[Value]) -> Result<(), DomainError> {
        let def = self.definition(id)?;
        let bytes = self.encode_physical_row(def, row)?;
        let key = key_from_row(def, row)?;
        self.inner.replace(&key, &bytes).map_err(map_mvcc)
    }

    pub fn delete(&mut self, id: TableId, pk: &[Value]) -> Result<(), DomainError> {
        let def = self.definition(id)?;
        let key = encode_key(&def.table, &def.pk_positions, pk)?;
        self.inner.delete(&key).map_err(map_mvcc)
    }

    pub fn rollback(self) { self.inner.rollback(); }

    pub fn commit(self) -> Result<u64, DomainError> {
        self.inner.commit().map_err(map_mvcc)
    }
}

#[cfg(test)]
mod activation_tests {
    use super::*;
    use crate::{Column, DataType, PrimaryKey, TableSpec};
    use crate::history_sql::HistorySqlEngine;
    use crate::sql::SqlResultSet;
    use tempfile::{tempdir, TempDir};

    fn fixture() -> (TempDir, Table) {
        let dir = tempdir().unwrap();
        let catalog = PersistentCatalog::open(dir.path().join("catalog")).unwrap();
        let db = catalog.create_database("test").unwrap();
        let schema = catalog.create_schema(db, "public").unwrap();
        let id = catalog.create_table(schema, TableSpec {
            name: "items".into(), columns: vec![
                Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
                Column { name: "amount".into(), data_type: DataType::Int64, nullable: false },
            ], primary_key: PrimaryKey { columns: vec!["id".into()] }, unique_keys: vec![],
        }).unwrap();
        let table = catalog.snapshot().table(id).unwrap().clone();
        let raw = DurableMvccStore::open(dir.path().join("history-domain")).unwrap();
        let mut tx = raw.begin();
        let mut schema_key = SCHEMA_PREFIX.to_vec();
        schema_key.extend_from_slice(&id.0.to_be_bytes());
        tx.insert(&schema_key, &serde_json::to_vec(&table).unwrap()).unwrap();
        tx.commit().unwrap();
        let mut tx = raw.begin();
        tx.insert(&encode_key(&table, &[0], &[Value::Int64(1)]).unwrap(),
            &encode_row(&table, &[Value::Int64(1), Value::Int64(10)]).unwrap()).unwrap();
        tx.commit().unwrap();
        drop(raw);
        catalog.checkpoint().unwrap();
        drop(catalog);
        let engine = HistorySqlEngine::open(dir.path()).unwrap();
        engine.enable_epoch_row_writes().unwrap();
        engine.checkpoint().unwrap();
        drop(engine);
        let mut proposed = table;
        proposed.columns.push(Column { name: "note".into(), data_type: DataType::Text, nullable: true });
        (dir, proposed)
    }

    fn interrupted(stage: ActivationStage) -> (TempDir, Table) {
        interrupted_revision(stage, false)
    }

    fn interrupted_revision(stage: ActivationStage, relaxation: bool) -> (TempDir, Table) {
        let (dir, mut proposed) = fixture();
        if relaxation {
            proposed.columns.pop();
            proposed.columns[1].nullable = true;
        }
        let mut engine = HistorySqlEngine::open(dir.path()).unwrap();
        assert!(engine.activate_schema(proposed.clone(), &mut |current| {
            if current == stage { Err(DomainError::Corruption("injected interruption".into())) }
            else { Ok(()) }
        }).is_err());
        assert!(engine.with_session(|_| Ok(())).is_err());
        assert!(engine.checkpoint().is_err());
        assert!(engine.backup_to(dir.path().join("refused-backup")).is_err());
        assert!(engine.execute_ddl("ALTER TABLE test.public.items ADD extra TEXT").is_err());
        drop(engine);
        (dir, proposed)
    }

    fn assert_schema(root: &Path, width: usize) -> u64 {
        let engine = HistorySqlEngine::open(root).unwrap();
        let (schema, _) = engine.schema_as_of("test.public.items", engine.last_lsn()).unwrap();
        assert_eq!(schema.columns.len(), width);
        engine.with_session(|s| {
            s.execute("BEGIN")?;
            match s.execute("SELECT * FROM test.public.items WHERE id=1")? {
                SqlResultSet::Rows { rows, .. } => {
                    assert_eq!(rows[0].len(), width);
                    assert_eq!(rows[0][1], Value::Int64(10));
                    if width == 3 { assert_eq!(rows[0][2], Value::Null); }
                },
                other => panic!("expected row: {other:?}"),
            }
            Ok(())
        }).unwrap();
        engine.last_lsn()
    }

    #[test]
    fn each_durable_boundary_rolls_forward_and_reopen_is_idempotent() {
        for stage in [ActivationStage::IntentSynced, ActivationStage::CatalogProposed,
            ActivationStage::CatalogActivated, ActivationStage::HistoryActivated]
        {
            let (dir, proposed) = interrupted(stage);
            let lsn = assert_schema(dir.path(), 3);
            assert_eq!(assert_schema(dir.path(), 3), lsn);
            let catalog = PersistentCatalog::open(dir.path().join("catalog")).unwrap();
            assert_eq!(catalog.snapshot().table(proposed.id), Some(&proposed));
            assert!(catalog.proposed_table_revisions().is_empty());
            assert_eq!(catalog.last_lsn(), 6);
        }
    }

    fn last_frame_offset(bytes: &[u8]) -> usize {
        let mut pos = 0;
        let mut last = 0;
        while pos < bytes.len() {
            last = pos;
            assert_eq!(&bytes[pos..pos + 4], b"BRWL");
            pos += 12 + u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        }
        assert_eq!(pos, bytes.len());
        last
    }

    #[test]
    fn torn_frames_at_all_four_boundaries_repair_before_recovery() {
        for stage in [ActivationStage::IntentSynced, ActivationStage::CatalogProposed,
            ActivationStage::CatalogActivated, ActivationStage::HistoryActivated]
        {
            for cut in [0_usize, 1, 17, usize::MAX] {
                let (dir, _) = interrupted(stage);
                let component = if matches!(stage, ActivationStage::CatalogProposed | ActivationStage::CatalogActivated) {
                    "catalog"
                } else { "history-domain" };
                let path = dir.path().join(component).join("rows.wal");
                let bytes = std::fs::read(&path).unwrap();
                let offset = last_frame_offset(&bytes);
                let length = offset + cut.min(bytes.len() - offset - 1);
                let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
                file.set_len(length as u64).unwrap();
                file.sync_all().unwrap();
                drop(file);
                let width = if stage == ActivationStage::IntentSynced { 2 } else { 3 };
                let lsn = assert_schema(dir.path(), width);
                assert_eq!(assert_schema(dir.path(), width), lsn);
            }
        }
    }

    #[test]
    fn recovery_refuses_catalog_divergence_without_mutating_either_store() {
        let (dir, _) = interrupted(ActivationStage::IntentSynced);
        let catalog = PersistentCatalog::open(dir.path().join("catalog")).unwrap();
        catalog.create_database("unexpected").unwrap();
        let catalog_lsn = catalog.last_lsn();
        drop(catalog);
        let raw = DurableMvccStore::open(dir.path().join("history-domain")).unwrap();
        let history_lsn = raw.latest_lsn();
        drop(raw);
        assert!(HistorySqlEngine::open(dir.path()).is_err());
        assert_eq!(PersistentCatalog::open(dir.path().join("catalog")).unwrap().last_lsn(), catalog_lsn);
        assert_eq!(DurableMvccStore::open(dir.path().join("history-domain")).unwrap().latest_lsn(), history_lsn);
    }

    #[test]
    fn history_writes_after_pending_intent_are_not_treated_as_recoverable_ddl() {
        let (dir, _) = interrupted(ActivationStage::IntentSynced);
        let raw = DurableMvccStore::open(dir.path().join("history-domain")).unwrap();
        let mut tx = raw.begin();
        tx.insert(b"unexpected", b"uncoordinated").unwrap();
        tx.commit().unwrap();
        drop(raw);
        assert!(HistorySqlEngine::open(dir.path()).is_err());
        assert_eq!(PersistentCatalog::open(dir.path().join("catalog")).unwrap().last_lsn(), 4);
    }

    #[test]
    fn standalone_adapter_refuses_unfinished_activations() {
        let (dir, _) = interrupted(ActivationStage::CatalogProposed);
        let catalog = PersistentCatalog::open(dir.path().join("catalog")).unwrap();
        let definitions = catalog.snapshot().tables().map(|t| (t.id, t.clone())).collect();
        assert!(DurableTypedStore::open(dir.path().join("history-domain"), &definitions).is_err());
    }

    #[test]
    fn forged_intents_are_validated_before_any_catalog_repair() {
        for defect in ["version", "previous_epoch", "name", "type", "table", "catalog", "overflow"] {
            let (dir, mut proposed) = fixture();
            let catalog = PersistentCatalog::open(dir.path().join("catalog")).unwrap();
            let definitions = catalog.snapshot().tables().map(|t| (t.id, t.clone())).collect();
            let domain = DurableTypedStore::open(dir.path().join("history-domain"), &definitions).unwrap();
            let mut plan = domain.plan_schema_activation(&catalog, proposed.clone()).unwrap();
            match defect {
                "version" => plan.intent.version = 2,
                "previous_epoch" => plan.intent.previous_epoch += 100,
                "name" => plan.intent.proposed.columns.last_mut().unwrap().name = "Bad Name".into(),
                "type" => plan.intent.proposed.columns.last_mut().unwrap().data_type = DataType::Varchar { max_length: 0 },
                "table" => { proposed.id = TableId(999); plan.intent.proposed = proposed; },
                "catalog" => plan.intent.catalog_before_lsn = 999,
                "overflow" => plan.intent.catalog_before_lsn = u64::MAX,
                _ => unreachable!(),
            }
            drop(domain);
            drop(catalog);
            let raw = DurableMvccStore::open(dir.path().join("history-domain")).unwrap();
            let mut tx = raw.begin();
            tx.insert(&schema_activation::intent_key(plan.intent_lsn),
                &serde_json::to_vec(&plan.intent).unwrap()).unwrap();
            tx.commit().unwrap();
            drop(raw);
            assert!(HistorySqlEngine::open(dir.path()).is_err(), "accepted {defect}");
            assert_eq!(PersistentCatalog::open(dir.path().join("catalog")).unwrap().last_lsn(), 4);
        }
    }
    #[test]
    fn completed_activation_records_cannot_be_rewritten_or_deleted() {
        for deleted in [false, true] {
            let (dir, _) = interrupted(ActivationStage::HistoryActivated);
            let raw = DurableMvccStore::open(dir.path().join("history-domain")).unwrap();
            let (key, _, value) = raw.retained_versions().into_iter().find(|(key, _, _)|
                schema_activation::is_activation_key(key) && key != &schema_activation::intent_key(5)).unwrap();
            let mut tx = raw.begin();
            if deleted { tx.delete(&key).unwrap(); }
            else { tx.replace(&key, &value.unwrap()).unwrap(); }
            tx.commit().unwrap();
            drop(raw);
            assert!(HistorySqlEngine::open(dir.path()).is_err());
            assert_eq!(PersistentCatalog::open(dir.path().join("catalog")).unwrap().last_lsn(), 6);
        }
    }

    #[test]
    fn forged_activation_commits_do_not_trigger_catalog_repairs() {
        for defect in ["no_intent", "wrong_catalog", "wrong_identity", "catalog_not_committed"] {
            let (dir, _) = if defect == "no_intent" { fixture() }
                else { interrupted(ActivationStage::IntentSynced) };
            let raw = DurableMvccStore::open(dir.path().join("history-domain")).unwrap();
            let intent_lsn = if defect == "no_intent" { 4 } else { 5 };
            let record = ActivationCommit { version: 1,
                intent_lsn: if defect == "wrong_identity" { intent_lsn + 1 } else { intent_lsn },
                catalog_lsn: if defect == "wrong_catalog" { 7 } else { 6 } };
            let mut tx = raw.begin();
            tx.insert(&schema_activation::activation_key(intent_lsn), &serde_json::to_vec(&record).unwrap()).unwrap();
            tx.commit().unwrap();
            drop(raw);
            assert!(HistorySqlEngine::open(dir.path()).is_err(), "accepted {defect}");
            assert_eq!(PersistentCatalog::open(dir.path().join("catalog")).unwrap().last_lsn(), 4);
        }
    }

    #[test]
    fn complete_wal_crc_corruption_is_not_treated_as_an_incomplete_tail() {
        let (dir, _) = interrupted(ActivationStage::CatalogProposed);
        let path = dir.path().join("catalog/rows.wal");
        let mut bytes = std::fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 0x80;
        std::fs::write(&path, bytes).unwrap();
        assert!(HistorySqlEngine::open(dir.path()).is_err());
        let raw = DurableMvccStore::open(dir.path().join("history-domain")).unwrap();
        assert_eq!(raw.latest_lsn(), 5); // intent remains, no false completion
    }

    #[test]
    fn forged_retirement_receipts_do_not_authorize_catalog_horizon_changes() {
        for defect in ["version", "count", "digest", "horizon"] {
            let (dir, proposed) = fixture();
            let catalog = PersistentCatalog::open(dir.path().join("catalog")).unwrap();
            let tables = catalog.snapshot().tables().map(|t| (t.id, t.clone())).collect();
            let domain = DurableTypedStore::open(dir.path().join("history-domain"), &tables).unwrap();
            domain.prepare_nullable_addition(proposed.id, &proposed).unwrap();
            let (records, crc32) = schema_activation::retirement_digest(&domain.store.retained_versions()).unwrap();
            let mut record = LegacyDdlRetirement { version: 1, records, crc32, catalog_lsn: 4 };
            match defect {
                "version" => record.version = 2,
                "count" => record.records += 1,
                "digest" => record.crc32 ^= 1,
                "horizon" => record.catalog_lsn = 999,
                _ => unreachable!(),
            }
            let mut tx = domain.store.begin();
            tx.insert(schema_activation::retirement_key(), &serde_json::to_vec(&record).unwrap()).unwrap();
            tx.commit().unwrap();
            drop(domain);
            drop(catalog);
            assert!(HistorySqlEngine::open(dir.path()).is_err(), "accepted {defect}");
            assert_eq!(PersistentCatalog::open(dir.path().join("catalog")).unwrap().last_lsn(), 4);
        }
    }

    #[test]
    fn nullability_relaxation_recovers_at_every_durable_boundary() {
        for stage in [ActivationStage::IntentSynced, ActivationStage::CatalogProposed,
            ActivationStage::CatalogActivated, ActivationStage::HistoryActivated]
        {
            let (dir, proposed) = interrupted_revision(stage, true);
            let engine = HistorySqlEngine::open(dir.path()).unwrap();
            assert_eq!(engine.schema_as_of("test.public.items", engine.last_lsn()).unwrap().0, proposed);
            assert!(!engine.schema_as_of("test.public.items", 4).unwrap().0.columns[1].nullable);
            engine.with_session(|s| {
                s.execute("BEGIN")?;
                s.execute("UPDATE test.public.items SET amount=NULL WHERE id=1")?;
                s.execute("COMMIT")?;
                s.execute("BEGIN")?;
                assert!(matches!(s.execute("SELECT amount FROM test.public.items AS OF 4 WHERE id=1")?,
                    SqlResultSet::Rows { rows, .. } if rows == vec![vec![Value::Int64(10)]]));
                Ok(())
            }).unwrap();
            let lsn = engine.last_lsn();
            drop(engine);
            assert_eq!(HistorySqlEngine::open(dir.path()).unwrap().last_lsn(), lsn);
        }
    }

    #[test]
    fn torn_nullability_transition_frames_repair_at_all_four_boundaries() {
        for stage in [ActivationStage::IntentSynced, ActivationStage::CatalogProposed,
            ActivationStage::CatalogActivated, ActivationStage::HistoryActivated]
        {
            for cut in [0_usize, 1, 17, usize::MAX] {
                let (dir, _) = interrupted_revision(stage, true);
                let component = if matches!(stage, ActivationStage::CatalogProposed | ActivationStage::CatalogActivated) {
                    "catalog"
                } else { "history-domain" };
                let path = dir.path().join(component).join("rows.wal");
                let bytes = std::fs::read(&path).unwrap();
                let offset = last_frame_offset(&bytes);
                let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
                file.set_len((offset + cut.min(bytes.len() - offset - 1)) as u64).unwrap();
                file.sync_all().unwrap();
                drop(file);
                let engine = HistorySqlEngine::open(dir.path()).unwrap();
                let nullable = stage != ActivationStage::IntentSynced;
                assert_eq!(engine.schema_as_of("test.public.items", engine.last_lsn()).unwrap().0.columns[1].nullable, nullable);
                let lsn = engine.last_lsn();
                drop(engine);
                assert_eq!(HistorySqlEngine::open(dir.path()).unwrap().last_lsn(), lsn);
            }
        }
    }

}
