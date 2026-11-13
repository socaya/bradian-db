//! G36 immutable, replay-validated coordination records for exclusive ALTER.
use std::collections::{BTreeMap, BTreeSet};
use serde::{Deserialize, Serialize};
use crate::{Table, TableId};
use crate::transaction_domain::DomainError;

const INTENT: &[u8] = b"\0bradian-schema-activate-intent-v1:";
const RETIREMENT: &[u8] = b"\0bradian-schema-legacy-retirement-v1";
const ACTIVE: &[u8] = b"\0bradian-schema-activate-commit-v1:";
pub(crate) type SchemaTimeline = BTreeMap<TableId, BTreeMap<u64, Table>>;
type RetainedVersion = (Vec<u8>, u64, Option<Vec<u8>>);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActivationIntent {
    pub version: u8,
    pub previous_epoch: u64,
    pub catalog_before_lsn: u64,
    pub proposed: Table,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActivationCommit {
    pub version: u8,
    pub intent_lsn: u64,
    pub catalog_lsn: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingActivation {
    pub intent_lsn: u64,
    pub intent: ActivationIntent,
}
impl PendingActivation {
    pub fn catalog_lsn(&self) -> Result<u64, DomainError> {
        self.intent.catalog_before_lsn.checked_add(2)
            .ok_or_else(|| invalid("catalog horizon overflow"))
    }
}

/// A completed local DDL decision. Epoch is the final history WAL commit LSN.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchemaActivationReceipt {
    pub table_id: TableId,
    pub intent_lsn: u64,
    pub schema_epoch: u64,
    pub catalog_lsn: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyDdlRetirement {
    pub version: u8,
    pub catalog_lsn: u64,
    pub records: u64,
    pub crc32: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegacyDdlRetirementReceipt {
    pub history_lsn: u64,
    pub catalog_lsn: u64,
    pub retired_records: u64,
}

pub(crate) fn retirement_key() -> &'static [u8] { RETIREMENT }

pub(crate) fn retirement_digest(versions: &[RetainedVersion]) -> Result<(u64, u32), DomainError> {
    let legacy: Vec<_> = versions.iter().filter(|(key, _, _)|
        crate::durable_typed::is_legacy_ddl_key(key)).collect();
    Ok((legacy.len() as u64, crc32fast::hash(&serde_json::to_vec(&legacy)?)))
}

pub(crate) fn load_retirement(
    versions: &[RetainedVersion],
) -> Result<Option<(LegacyDdlRetirement, u64)>, DomainError> {
    let mut result = None;
    for (key, lsn, value) in versions {
        if key.as_slice() != RETIREMENT { continue; }
        if result.is_some() { return Err(invalid("legacy retirement was rewritten")); }
        let bytes = value.as_deref().ok_or_else(|| invalid("legacy retirement was deleted"))?;
        let record: LegacyDdlRetirement = serde_json::from_slice(bytes)?;
        let (count, digest) = retirement_digest(versions)?;
        if record.version != 1 || record.catalog_lsn == 0 || count == 0
            || record.records != count || record.crc32 != digest
            || versions.iter().any(|(key, old_lsn, _)|
                (crate::durable_typed::is_legacy_ddl_key(key) && old_lsn >= lsn)
                    || (old_lsn == lsn && key.as_slice() != RETIREMENT))
        { return Err(invalid("legacy retirement digest or chronology differs")); }
        result = Some((record, *lsn));
    }
    Ok(result)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActivationStage {
    IntentSynced, CatalogProposed, CatalogActivated, HistoryActivated,
}

pub(crate) struct SchemaChanges {
    pub timeline: SchemaTimeline,
    pub pending: Option<PendingActivation>,
    pub catalog_horizon: Option<u64>,
}

pub(crate) fn is_activation_key(key: &[u8]) -> bool {
    key.starts_with(INTENT) || key.starts_with(ACTIVE)
}
fn key(prefix: &[u8], intent_lsn: u64) -> Vec<u8> {
    let mut key = prefix.to_vec();
    key.extend_from_slice(&intent_lsn.to_be_bytes());
    key
}
pub(crate) fn intent_key(lsn: u64) -> Vec<u8> { key(INTENT, lsn) }
pub(crate) fn activation_key(lsn: u64) -> Vec<u8> { key(ACTIVE, lsn) }
fn invalid(message: &str) -> DomainError {
    DomainError::Corruption(format!("schema activation: {message}"))
}

pub(crate) fn validate_nullable_evolution(old: &Table, proposed: &Table) -> Result<(), DomainError> {
    crate::schema_evolution::plan_nullable_evolution(old, proposed)?;
    crate::catalog::validate_revision_columns(proposed)
        .map_err(|err| invalid(&err.to_string()))?;
    Ok(())
}

enum Event { Intent(ActivationIntent), Active(ActivationCommit) }
impl SchemaChanges {
    pub(crate) fn load(
        versions: &[RetainedVersion], mut timeline: SchemaTimeline,
        encoding_lsn: Option<u64>, latest: u64, legacy_ddl: bool,
        initial_catalog_horizon: Option<u64>,
    ) -> Result<Self, DomainError> {
        let mut events = BTreeMap::new();
        let mut keys = BTreeSet::new();
        let mut event_keys = BTreeMap::new();
        for (key, lsn, value) in versions {
            if !is_activation_key(key) { continue; }
            if legacy_ddl || encoding_lsn.is_none_or(|enabled| enabled >= *lsn) {
                return Err(invalid("requires epoch encoding and no legacy DDL intents"));
            }
            if !keys.insert(key.clone()) { return Err(invalid("immutable record was rewritten")); }
            let bytes = value.as_deref().ok_or_else(|| invalid("immutable record was deleted"))?;
            let (prefix, intent) = if key.starts_with(INTENT) { (INTENT, true) } else { (ACTIVE, false) };
            if key.len() != prefix.len() + 8 { return Err(invalid("malformed record key")); }
            let id = u64::from_be_bytes(key[prefix.len()..].try_into().unwrap());
            let event = if intent {
                if id != *lsn { return Err(invalid("intent identity differs from WAL position")); }
                Event::Intent(serde_json::from_slice(bytes)?)
            } else {
                let commit: ActivationCommit = serde_json::from_slice(bytes)?;
                if commit.intent_lsn != id { return Err(invalid("activation identity mismatch")); }
                Event::Active(commit)
            };
            if events.insert(*lsn, event).is_some() { return Err(invalid("multiple events in one commit")); }
            event_keys.insert(*lsn, key);
        }
        for (key, lsn, _) in versions {
            if event_keys.get(lsn).is_some_and(|expected| *expected != key) {
                return Err(invalid("row or metadata writes inside a DDL commit"));
            }
        }
        let mut pending: Option<PendingActivation> = None;
        let mut catalog_horizon = initial_catalog_horizon;
        for (lsn, event) in events {
            match event {
                Event::Intent(intent) => {
                    if pending.is_some() || intent.version != 1 || intent.catalog_before_lsn == 0
                        || catalog_horizon.is_some_and(|prior| prior != intent.catalog_before_lsn)
                    { return Err(invalid("invalid or overlapping intent chronology")); }
                    let schemas = timeline.get(&intent.proposed.id)
                        .ok_or_else(|| invalid("intent references an unregistered table"))?;
                    let (epoch, old) = schemas.last_key_value().expect("registered schema timeline");
                    if *epoch != intent.previous_epoch { return Err(invalid("stale previous schema epoch")); }
                    validate_nullable_evolution(old, &intent.proposed)?;
                    let candidate = PendingActivation { intent_lsn: lsn, intent };
                    candidate.catalog_lsn()?;
                    pending = Some(candidate);
                }
                Event::Active(commit) => {
                    let intent = pending.take().ok_or_else(|| invalid("activation without intent"))?;
                    if commit.version != 1 || commit.intent_lsn != intent.intent_lsn
                        || intent.intent_lsn.checked_add(1) != Some(lsn)
                        || commit.catalog_lsn != intent.catalog_lsn()?
                    { return Err(invalid("activation chronology or catalog horizon mismatch")); }
                    timeline.get_mut(&intent.intent.proposed.id).expect("validated table")
                        .insert(lsn, intent.intent.proposed);
                    catalog_horizon = Some(commit.catalog_lsn);
                }
            }
        }
        if pending.as_ref().is_some_and(|record| record.intent_lsn != latest) {
            return Err(invalid("history changed after an unfinished schema intent"));
        }
        Ok(Self { timeline, pending, catalog_horizon })
    }
}
