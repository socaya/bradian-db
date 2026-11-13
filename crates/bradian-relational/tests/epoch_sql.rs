//! G35 live mixed-format SQL, explicit capability activation and recovery.
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Barrier};

use bradian_relational::{Column, DataType, Table, TableId};
use bradian_relational::durable_mvcc::DurableMvccStore;
use bradian_relational::durable_typed::DurableTypedStore;
use bradian_relational::epoch_row::EpochSchemas;
use bradian_relational::history_backup::restore_history_database;
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::persistent_catalog::PersistentCatalog;
use bradian_relational::typed_table::{decode_row, encode_row, Value};

#[path = "support/epoch_fixture.rs"]
mod fixture;
use fixture::{migrated, modify, query};

const ENCODING_KEY: &[u8] = b"\0bradian-row-encoding-v1";
const ENCODING_VALUE: &[u8] = b"BRADIAN-EPOCH-ROWS-V1";

fn definitions(root: &Path) -> BTreeMap<TableId, Table> {
    PersistentCatalog::open(root.join("catalog")).unwrap().snapshot().tables()
        .map(|t| (t.id, t.clone())).collect()
}

fn row(value: i64) -> Vec<Value> { vec![Value::Int64(value)] }

#[test]
fn epoch_mode_is_explicit_and_does_not_rewrite_legacy_rows_or_catalog() {
    let (_dir, history) = migrated();
    let original_catalog = definitions(&history);
    let original = {
        let raw = DurableMvccStore::open(history.join("history-domain")).unwrap();
        raw.scan_as_of(raw.latest_lsn()).unwrap()
    };
    let db = HistorySqlEngine::open(&history).unwrap();
    let before = db.last_lsn();
    let epochs = db.schema_epochs();
    assert!(!db.epoch_row_writes_enabled().unwrap());
    assert_eq!(db.last_lsn(), before);
    let activated = db.enable_epoch_row_writes().unwrap();
    assert_eq!(activated, before + 1);
    assert!(db.epoch_row_writes_enabled().unwrap());
    assert!(db.enable_epoch_row_writes().is_err());
    assert_eq!(db.last_lsn(), activated);
    assert_eq!(db.schema_epochs(), epochs);
    drop(db);
    assert_eq!(definitions(&history), original_catalog);
    let raw = DurableMvccStore::open(history.join("history-domain")).unwrap();
    let records = raw.scan_as_of(raw.latest_lsn()).unwrap();
    assert_eq!(records.iter().filter(|(k, _)| k.as_slice() != ENCODING_KEY).cloned().collect::<Vec<_>>(), original);
    assert_eq!(records.iter().filter(|(k, _)| k.as_slice() == ENCODING_KEY).count(), 1);
}

#[test]
fn mixed_row_sql_crud_as_of_checkpoint_restart_and_epoch_binding() {
    let (_dir, history) = migrated();
    let db = HistorySqlEngine::open(&history).unwrap();
    let original_lsn = db.last_lsn();
    let (table, epoch) = db.schema_as_of("bank.public.accounts", original_lsn).unwrap();
    let mut registry = EpochSchemas::default();
    registry.register(epoch, table.clone()).unwrap();
    db.enable_epoch_row_writes().unwrap();
    let updated = modify(&db, &[
        "UPDATE bank.public.accounts SET balance=110 WHERE id=1",
        "INSERT INTO bank.public.accounts(id,balance) VALUES(3,300)",
        "UPDATE bank.public.audit SET balance=95 WHERE id=9",
    ]);
    assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts"), vec![row(110), row(200), row(300)]);
    assert_eq!(query(&db, &format!("SELECT balance FROM bank.public.accounts AS OF {original_lsn}")), vec![row(100), row(200)]);
    assert_eq!(query(&db, &format!("SELECT balance FROM bank.public.accounts AS OF {updated} WHERE id=1")), vec![row(110)]);
    let deleted = modify(&db, &[
        "DELETE FROM bank.public.accounts WHERE id=2",
        "DELETE FROM bank.public.accounts WHERE id=3",
    ]);
    assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts"), vec![row(110)]);
    db.checkpoint().unwrap();
    drop(db);
    {
        let raw = DurableMvccStore::open(history.join("history-domain")).unwrap();
        let original = raw.scan_as_of(original_lsn).unwrap();
        let mut new_rows = 0;
        let mut old_rows = 0;
        let mut accounts_rows = 0;
        for (key, bytes) in raw.scan_as_of(updated).unwrap() {
            if !key.starts_with(b"BRPK") { continue; }
            if bytes.starts_with(b"BRVE") {
                new_rows += 1;
                if let Ok(decoded) = registry.decode(&bytes) {
                    assert_eq!(decoded.epoch, epoch);
                    assert_eq!(decoded.table_id, table.id);
                    accounts_rows += 1;
                }
            } else { assert!(bytes.starts_with(b"BRRW")); old_rows += 1; }
        }
        assert_eq!((new_rows, old_rows, accounts_rows), (3, 1, 2));
        assert_eq!(original.iter().filter(|(k, _)| k.starts_with(b"BRPK")).count(), 3);
        assert!(original.iter().filter(|(k, _)| k.starts_with(b"BRPK")).all(|(_, v)| v.starts_with(b"BRRW")));
        assert_eq!(raw.scan_as_of(deleted).unwrap().iter().filter(|(k, _)| k.starts_with(b"BRPK")).count(), 2);
    }
    let db = HistorySqlEngine::open(&history).unwrap();
    assert!(db.epoch_row_writes_enabled().unwrap());
    assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![row(110)]);
    assert_eq!(query(&db, &format!("SELECT balance FROM bank.public.accounts AS OF {original_lsn} WHERE id=1")), vec![row(100)]);
    assert_eq!(query(&db, &format!("SELECT balance FROM bank.public.accounts AS OF {updated}")), vec![row(110), row(200), row(300)]);
}

#[test]
fn transactions_spanning_activation_keep_their_pinned_encoding_mode() {
    let (_dir, history) = migrated();
    let definitions = definitions(&history);
    let id = definitions.values().find(|t| t.name == "accounts").unwrap().id;
    let store = DurableTypedStore::open(history.join("history-domain"), &definitions).unwrap();
    let mut old = store.begin();
    old.replace(id, &[Value::Int64(1), Value::Int64(101)]).unwrap();
    store.enable_epoch_row_writes().unwrap();
    let mut new = store.begin();
    new.insert(id, &[Value::Int64(3), Value::Int64(303)]).unwrap();
    let new_lsn = new.commit().unwrap();
    let old_lsn = old.commit().unwrap();
    assert!(old_lsn > new_lsn);
    assert_eq!(store.begin().scan(id).unwrap().len(), 3);
    drop(store);
    let raw = DurableMvccStore::open(history.join("history-domain")).unwrap();
    let records = raw.scan_as_of(old_lsn).unwrap();
    assert_eq!(records.iter().filter(|(k, v)| k.starts_with(b"BRPK") && v.starts_with(b"BRRW")).count(), 3);
    assert_eq!(records.iter().filter(|(k, v)| k.starts_with(b"BRPK") && v.starts_with(b"BRVE")).count(), 1);
    drop(raw);
    assert_eq!(query(&HistorySqlEngine::open(&history).unwrap(), "SELECT balance FROM bank.public.accounts"), vec![row(101), row(200), row(303)]);
}

#[test]
fn concurrent_opt_in_commits_exactly_one_format_decision() {
    let (_dir, history) = migrated();
    let db = Arc::new(HistorySqlEngine::open(&history).unwrap());
    let before = db.last_lsn();
    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8).map(|_| {
        let db = Arc::clone(&db);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || { barrier.wait(); db.enable_epoch_row_writes() })
    }).collect();
    let successes: Vec<_> = handles.into_iter().filter_map(|h| h.join().unwrap().ok()).collect();
    assert_eq!(successes, vec![before + 1]);
    assert!(db.epoch_row_writes_enabled().unwrap());
    assert_eq!(db.last_lsn(), before + 1);
    drop(db);
    assert!(HistorySqlEngine::open(&history).unwrap().epoch_row_writes_enabled().unwrap());
}

#[test]
fn backup_restore_preserves_epoch_mode_history_and_schema_intents() {
    let (dir, history) = migrated();
    let db = HistorySqlEngine::open(&history).unwrap();
    let old = db.last_lsn();
    let (mut proposed, _) = db.schema_as_of("bank.public.accounts", old).unwrap();
    db.enable_epoch_row_writes().unwrap();
    modify(&db, &["UPDATE bank.public.accounts SET balance=555 WHERE id=1"]);
    proposed.columns.push(Column { name: "note".into(), data_type: DataType::Text, nullable: true });
    db.prepare_nullable_addition("bank.public.accounts", &proposed).unwrap();
    db.coordinate_prepared_schema("bank.public.accounts").unwrap();
    db.certify_schema_readiness("bank.public.accounts").unwrap();
    let horizon = db.last_lsn();
    db.backup_to(dir.path().join("archive")).unwrap();
    restore_history_database(dir.path().join("archive"), dir.path().join("restored")).unwrap();
    let restored = HistorySqlEngine::open(dir.path().join("restored")).unwrap();
    assert_eq!(restored.last_lsn(), horizon);
    assert!(restored.epoch_row_writes_enabled().unwrap());
    assert_eq!(restored.prepared_schema_proposals().len(), 1);
    assert_eq!(restored.ddl_readiness_records().len(), 1);
    assert_eq!(query(&restored, &format!("SELECT balance FROM bank.public.accounts AS OF {old} WHERE id=1")), vec![row(100)]);
    assert_eq!(query(&restored, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![row(555)]);
    modify(&restored, &["INSERT INTO bank.public.accounts(id,balance) VALUES(5,500)"]);
    assert_eq!(query(&restored, "SELECT balance FROM bank.public.accounts WHERE id=5"), vec![row(500)]);
    // The original live store and independent restored store retain separate locks.
    assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=5"), Vec::<Vec<Value>>::new());
}

#[test]
fn envelope_capacity_and_statement_error_roll_back_the_entire_sql_batch() {
    let (_dir, history) = migrated();
    let db = HistorySqlEngine::open(&history).unwrap();
    let body = "x".repeat(3500);
    modify(&db, &[&format!("INSERT INTO bank.public.messages(id,body) VALUES(1,'{body}')")]);
    db.enable_epoch_row_writes().unwrap();
    let before = db.last_lsn();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE bank.public.accounts SET balance=999 WHERE id=1")?;
        assert!(s.execute(&format!("INSERT INTO bank.public.messages(id,body) VALUES(2,'{body}')")).is_err());
        assert!(!s.in_transaction());
        assert!(s.execute("COMMIT").is_err());
        Ok(())
    }).unwrap();
    assert_eq!(db.last_lsn(), before);
    assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![row(100)]);
    assert_eq!(query(&db, "SELECT body FROM bank.public.messages WHERE id=1"), vec![vec![Value::String(body)]]);
    let (table, epoch) = db.schema_as_of("bank.public.messages", before).unwrap();
    let overhead = encode_row(&table, &[Value::Int64(2), Value::String(String::new())]).unwrap().len() + 29;
    let exact = "e".repeat(3545 - overhead);
    let mut registry = EpochSchemas::default();
    registry.register(epoch, table.clone()).unwrap();
    assert_eq!(registry.encode(table.id, epoch,
        &[Value::Int64(2), Value::String(exact.clone())]).unwrap().len(), 3545);
    modify(&db, &[&format!("INSERT INTO bank.public.messages(id,body) VALUES(2,'{exact}')")]);
    assert_eq!(query(&db, "SELECT body FROM bank.public.messages WHERE id=2"), vec![vec![Value::String(exact)]]);
}

#[test]
fn epoch_metadata_survives_retention_and_old_horizons_remain_rejected() {
    let (_dir, history) = migrated();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.enable_epoch_row_writes().unwrap();
    modify(&db, &["UPDATE bank.public.accounts SET balance=111 WHERE id=1"]);
    let floor = modify(&db, &["UPDATE bank.public.accounts SET balance=222 WHERE id=1"]);
    drop(db);
    let raw = DurableMvccStore::open(history.join("history-domain")).unwrap();
    assert!(raw.prune_through(floor).unwrap() > 0);
    raw.checkpoint().unwrap();
    drop(raw);
    let db = HistorySqlEngine::open(&history).unwrap();
    assert!(db.epoch_row_writes_enabled().unwrap());
    assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![row(222)]);
    assert_eq!(query(&db, &format!("SELECT balance FROM bank.public.accounts AS OF {floor} WHERE id=1")), vec![row(222)]);
    db.with_session(|s| {
        s.execute("BEGIN")?;
        assert!(s.execute(&format!("SELECT balance FROM bank.public.accounts AS OF {}", floor - 1)).is_err());
        Ok(())
    }).unwrap();
}

fn account_key(raw: &DurableMvccStore, table: &Table) -> Vec<u8> {
    raw.scan_as_of(raw.latest_lsn()).unwrap().into_iter().find_map(|(key, bytes)| {
        let row = decode_row(table, &bytes).ok()?;
        (row.first() == Some(&Value::Int64(1))).then_some(key)
    }).expect("legacy seed account")
}

#[test]
fn malformed_retained_envelopes_are_rejected_even_after_tombstoning() {
    for defect in ["checksum", "unknown_epoch", "zero_epoch", "wrong_table", "wrong_pk", "version", "length", "truncated"] {
        let (_dir, history) = migrated();
        let db = HistorySqlEngine::open(&history).unwrap();
        let (table, epoch) = db.schema_as_of("bank.public.accounts", db.last_lsn()).unwrap();
        let (other, other_epoch) = db.schema_as_of("bank.public.audit", db.last_lsn()).unwrap();
        db.enable_epoch_row_writes().unwrap();
        drop(db);
        let mut registry = EpochSchemas::default();
        registry.register(epoch, table.clone()).unwrap();
        registry.register(other_epoch, other.clone()).unwrap();
        let mut bytes = registry.encode(table.id, epoch, &[Value::Int64(1), Value::Int64(100)]).unwrap();
        match defect {
            "checksum" => *bytes.last_mut().unwrap() ^= 1,
            "unknown_epoch" => bytes[13..21].copy_from_slice(&(epoch + 1000).to_le_bytes()),
            "zero_epoch" => bytes[13..21].fill(0),
            "wrong_table" => bytes = registry.encode(other.id, other_epoch, &[Value::Int64(1), Value::Int64(100)]).unwrap(),
            "wrong_pk" => bytes = registry.encode(table.id, epoch, &[Value::Int64(7), Value::Int64(100)]).unwrap(),
            "version" => bytes[4] = 2,
            "length" => bytes[21] ^= 1,
            "truncated" => { bytes.pop(); },
            _ => unreachable!(),
        }
        let raw = DurableMvccStore::open(history.join("history-domain")).unwrap();
        let key = account_key(&raw, &table);
        let mut tx = raw.begin();
        tx.replace(&key, &bytes).unwrap();
        tx.commit().unwrap();
        let mut tx = raw.begin();
        tx.delete(&key).unwrap();
        tx.commit().unwrap();
        raw.checkpoint().unwrap();
        drop(raw);
        assert!(HistorySqlEngine::open(&history).is_err(), "accepted {defect}");
    }
}

#[test]
fn envelopes_cannot_precede_or_share_the_format_activation_commit() {
    for ordering in ["absent", "same_commit", "later_marker"] {
        let (_dir, history) = migrated();
        let db = HistorySqlEngine::open(&history).unwrap();
        let (table, epoch) = db.schema_as_of("bank.public.accounts", db.last_lsn()).unwrap();
        drop(db);
        let mut registry = EpochSchemas::default();
        registry.register(epoch, table.clone()).unwrap();
        let bytes = registry.encode(table.id, epoch, &[Value::Int64(1), Value::Int64(100)]).unwrap();
        let raw = DurableMvccStore::open(history.join("history-domain")).unwrap();
        let key = account_key(&raw, &table);
        let mut tx = raw.begin();
        tx.replace(&key, &bytes).unwrap();
        if ordering == "same_commit" { tx.insert(ENCODING_KEY, ENCODING_VALUE).unwrap(); }
        tx.commit().unwrap();
        if ordering == "later_marker" {
            let mut tx = raw.begin();
            tx.insert(ENCODING_KEY, ENCODING_VALUE).unwrap();
            tx.commit().unwrap();
        }
        drop(raw);
        assert!(HistorySqlEngine::open(&history).is_err(), "accepted {ordering}");
    }
}

#[test]
fn rewritten_deleted_unknown_or_malformed_encoding_metadata_fails_closed() {
    for defect in ["rewritten", "deleted", "unknown_value", "unknown_key"] {
        let (_dir, history) = migrated();
        let db = HistorySqlEngine::open(&history).unwrap();
        db.enable_epoch_row_writes().unwrap();
        drop(db);
        let raw = DurableMvccStore::open(history.join("history-domain")).unwrap();
        let mut tx = raw.begin();
        match defect {
            "rewritten" => tx.replace(ENCODING_KEY, ENCODING_VALUE).unwrap(),
            "deleted" => tx.delete(ENCODING_KEY).unwrap(),
            "unknown_value" => tx.replace(ENCODING_KEY, b"BRADIAN-EPOCH-ROWS-V999").unwrap(),
            "unknown_key" => {
                let mut key = ENCODING_KEY.to_vec();
                key.push(0);
                tx.insert(&key, ENCODING_VALUE).unwrap();
            },
            _ => unreachable!(),
        }
        tx.commit().unwrap();
        drop(raw);
        assert!(HistorySqlEngine::open(&history).is_err(), "accepted {defect}");
    }
}

#[test]
fn partial_activation_wal_is_repaired_before_a_retry() {
    for cut in [0_u64, 1, 4, 17, u64::MAX] {
        let (_dir, history) = migrated();
        let db = HistorySqlEngine::open(&history).unwrap();
        let before = db.last_lsn();
        db.checkpoint().unwrap();
        db.enable_epoch_row_writes().unwrap();
        drop(db);
        let wal = std::fs::OpenOptions::new().write(true).open(history.join("history-domain/rows.wal")).unwrap();
        wal.set_len(cut.min(wal.metadata().unwrap().len() - 1)).unwrap();
        wal.sync_all().unwrap();
        drop(wal);
        let db = HistorySqlEngine::open(&history).unwrap();
        assert!(!db.epoch_row_writes_enabled().unwrap());
        assert_eq!(db.last_lsn(), before);
        assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![row(100)]);
        assert_eq!(db.enable_epoch_row_writes().unwrap(), before + 1);
        modify(&db, &["UPDATE bank.public.accounts SET balance=123 WHERE id=1"]);
        drop(db);
        let db = HistorySqlEngine::open(&history).unwrap();
        assert!(db.epoch_row_writes_enabled().unwrap());
        assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![row(123)]);
    }
}
