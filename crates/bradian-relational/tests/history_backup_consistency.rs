//! G38 snapshot receipts remain bound to archived versions as source writes advance.
#[path = "support/epoch_fixture.rs"]
mod fixture;
use fixture::{migrated, modify, query};
use bradian_relational::durable_mvcc::DurableMvccStore;
use bradian_relational::history_backup::restore_history_database;
use bradian_relational::history_sql::HistorySqlEngine;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc};
use tempfile::tempdir;

#[test]
fn receipt_statistics_describe_the_archive_after_source_mutation() {
    let dir = tempdir().unwrap();
    let source = DurableMvccStore::open(dir.path().join("source")).unwrap();
    let mut tx = source.begin();
    tx.insert(b"key", b"before").unwrap();
    tx.commit().unwrap();
    let (receipt, stats) = source.backup_with_stats(dir.path().join("archive")).unwrap();
    let mut tx = source.begin();
    tx.replace(b"key", b"after").unwrap();
    tx.commit().unwrap();
    assert!(source.latest_lsn() > stats.latest_lsn);
    assert!(source.stats().persisted_versions > stats.persisted_versions);
    DurableMvccStore::restore_backup(dir.path().join("archive"), dir.path().join("restored")).unwrap();
    let restored = DurableMvccStore::open(dir.path().join("restored")).unwrap();
    assert_eq!(restored.latest_lsn(), receipt.last_lsn);
    assert_eq!(restored.stats().persisted_versions, stats.persisted_versions);
    assert_eq!(restored.stats().retention_floor, stats.retention_floor);
    assert_eq!(restored.begin().get(b"key").unwrap(), Some(b"before".to_vec()));
}

#[test]
fn history_archives_remain_self_consistent_with_concurrent_epoch_writes() {
    let (dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD memo TEXT").unwrap();
    let db = Arc::new(db);
    let done = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = mpsc::channel();
    let writer = {
        let db = Arc::clone(&db);
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let mut value = 100;
            while !done.load(Ordering::Acquire) && value < 5000 {
                value += 1;
                modify(&db, &[&format!("UPDATE bank.public.accounts SET balance={value} WHERE id=1")]);
                if value == 110 { ready_tx.send(()).unwrap(); }
                std::thread::yield_now();
            }
        })
    };
    ready_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    let mut receipts = Vec::new();
    for i in 0..3 {
        let archive = dir.path().join(format!("archive-{i}"));
        receipts.push((archive.clone(), db.backup_to(&archive).unwrap()));
    }
    done.store(true, Ordering::Release);
    writer.join().unwrap();
    for (i, (archive, report)) in receipts.into_iter().enumerate() {
        let destination = dir.path().join(format!("restored-{i}"));
        restore_history_database(archive, &destination).unwrap();
        let restored = HistorySqlEngine::open(destination).unwrap();
        assert_eq!(restored.last_lsn(), report.history.last_lsn);
        assert_eq!(restored.mvcc_stats().persisted_versions, report.retained_versions);
        assert_eq!(query(&restored, "SELECT balance,memo FROM bank.public.accounts WHERE id=1"),
            query(&db, &format!("SELECT balance,memo FROM bank.public.accounts AS OF {} WHERE id=1", report.history.last_lsn)));
    }
}
