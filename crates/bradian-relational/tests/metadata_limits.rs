#[path="support/epoch_fixture.rs"]
mod fixture;
use std::fs;
use bradian_relational::row_store::{RowStore,StoreError};
use bradian_relational::checkpoint_manifest::{self,ManifestError};
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::history_backup::{restore_history_database,restore_history_database_with_policies,HistoryBackupError};
use bradian_relational::database_backup::{restore_database,restore_database_with_metadata_limits,DatabaseBackupError};
use bradian_relational::runtime_limits::{MetadataLimits,HistoryLimits,ResourceError};
use bradian_relational::lifecycle::CatalogLifecycle;
use bradian_relational::persistent_catalog::PersistentCatalog;
use bradian_relational::sql::SqlEngine;
use bradian_relational::typed_table::Value;
fn cap(n:usize)->MetadataLimits { MetadataLimits { max_manifest_bytes:n } }
fn resource(e:StoreError) { assert!(matches!(e,StoreError::Resource(ResourceError::MetadataLimit { .. }))); }
fn resize(path:&std::path::Path,n:u64) { fs::OpenOptions::new().write(true).open(path).unwrap().set_len(n).unwrap(); }

#[test]
fn fixed_checkpoint_manifest_refuses_short_trailing_and_sparse_oversized_files() {
    let dir=tempfile::tempdir().unwrap(); assert!(checkpoint_manifest::load(dir.path()).unwrap().is_none());
    let db=RowStore::open(dir.path()).unwrap(); db.put(b"a",b"v").unwrap(); db.checkpoint().unwrap(); drop(db);
    let path=dir.path().join("checkpoint.current"); let valid=fs::read(&path).unwrap();
    for n in [0,39,41,1_u64<<30] {
        fs::write(&path,&valid).unwrap(); resize(&path,n);
        assert!(matches!(checkpoint_manifest::load(dir.path()),Err(ManifestError::Corruption("wrong manifest length")))); assert_eq!(fs::metadata(&path).unwrap().len(),n);
    }
    fs::write(&path,&valid).unwrap(); assert!(checkpoint_manifest::load(dir.path()).unwrap().is_some());
    let mut bad=valid; bad[39]^=1; fs::write(&path,bad).unwrap(); assert!(matches!(checkpoint_manifest::load(dir.path()),Err(ManifestError::Corruption(_))));
}

#[test]
fn fixed_backup_receipt_refuses_oversize_before_creating_restore_stage() {
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open(dir.path().join("source")).unwrap(); db.put(b"a",b"v").unwrap(); let archive=dir.path().join("archive"); db.backup_to(&archive).unwrap();
    let path=archive.join("backup.info"); let valid=fs::read(&path).unwrap();
    for n in [0,39,41,1_u64<<30] {
        fs::write(&path,&valid).unwrap(); resize(&path,n); let dest=dir.path().join(format!("refused{n}"));
        assert!(matches!(RowStore::restore_backup(&archive,&dest),Err(StoreError::Corruption(_)))); assert!(!dest.exists()); assert!(!dir.path().join(format!(".refused{n}.bradian-incomplete")).exists());
        assert_eq!(fs::metadata(&path).unwrap().len(),n);
    }
    fs::write(&path,valid).unwrap(); RowStore::restore_backup(&archive,dir.path().join("valid")).unwrap();
}

#[test]
fn history_manifest_write_and_restore_use_same_exact_byte_policy() {
    let (dir,root)=fixture::migrated(); let db=HistorySqlEngine::open(&root).unwrap(); fixture::modify(&db,&["UPDATE bank.public.accounts SET balance=123 WHERE id=1"]);
    let refused=dir.path().join("refused"); match db.backup_to_with_metadata_limits(&refused,cap(0)).unwrap_err() { HistoryBackupError::Store(e)=>resource(e),e=>panic!("{e}") }
    assert!(!refused.exists()); assert!(!dir.path().join(".refused.bradian-g24-incomplete/history-backup.json").exists());
    assert_eq!(fixture::query(&db,"SELECT balance FROM bank.public.accounts WHERE id=1"),vec![vec![Value::Int64(123)]]);
    let archive=dir.path().join("archive"); db.backup_to(&archive).unwrap(); let bytes=fs::read(archive.join("history-backup.json")).unwrap(); let exact=cap(bytes.len());
    let archive2=dir.path().join("exact"); db.backup_to_with_metadata_limits(&archive2,exact).unwrap(); assert_eq!(fs::read(archive2.join("history-backup.json")).unwrap(),bytes);
    let refused=dir.path().join("refused-restore"); match restore_history_database_with_policies(&archive,&refused,HistoryLimits::default(),cap(bytes.len()-1)).unwrap_err() { HistoryBackupError::Store(e)=>resource(e),e=>panic!("{e}") }
    assert!(!refused.exists()); assert!(!dir.path().join(".refused-restore.bradian-g24-incomplete").exists());
    let restored=dir.path().join("restored"); restore_history_database_with_policies(&archive,&restored,HistoryLimits::default(),exact).unwrap(); let restored=HistorySqlEngine::open(&restored).unwrap();
    assert_eq!(fixture::query(&restored,"SELECT balance FROM bank.public.accounts WHERE id=1"),vec![vec![Value::Int64(123)]]);
}

fn lifecycle(root:&std::path::Path)->CatalogLifecycle {
    { let db=SqlEngine::open(root).unwrap(); for sql in ["CREATE DATABASE test","CREATE SCHEMA test.public","CREATE TABLE test.public.items(id BIGINT NOT NULL, PRIMARY KEY(id))"] { db.execute(sql).unwrap(); } }
    let id={ let catalog=PersistentCatalog::open(root.join("catalog")).unwrap(); let id=catalog.snapshot().tables().next().unwrap().id; id };
    let owner=CatalogLifecycle::open(root).unwrap(); owner.provision_existing(id).unwrap(); owner
}
#[test]
fn lifecycle_manifest_write_and_restore_use_same_exact_byte_policy() {
    let dir=tempfile::tempdir().unwrap(); let owner=lifecycle(&dir.path().join("source"));
    let refused=dir.path().join("refused"); match owner.backup_database_to_with_metadata_limits(&refused,cap(0)).unwrap_err() { DatabaseBackupError::Storage(e)=>resource(e),e=>panic!("{e}") }
    assert!(!refused.exists());
    let archive=dir.path().join("archive"); owner.backup_database_to(&archive).unwrap(); let bytes=fs::read(archive.join("database-backup.json")).unwrap();
    let archive2=dir.path().join("exact"); owner.backup_database_to_with_metadata_limits(&archive2,cap(bytes.len())).unwrap(); assert_eq!(fs::read(archive2.join("database-backup.json")).unwrap(),bytes);
    let refused=dir.path().join("refused-restore"); match restore_database_with_metadata_limits(&archive,&refused,cap(bytes.len()-1)).unwrap_err() { DatabaseBackupError::Storage(e)=>resource(e),e=>panic!("{e}") }
    assert!(!refused.exists()); let restored=dir.path().join("restored"); restore_database_with_metadata_limits(&archive,&restored,cap(bytes.len())).unwrap(); assert_eq!(CatalogLifecycle::open(&restored).unwrap().reconcile().unwrap().len(),1);
}

#[test]
fn default_json_limits_refuse_sparse_oversized_archives_before_staging_or_crc_work() {
    let dir=tempfile::tempdir().unwrap(); let history=dir.path().join("history"); let database=dir.path().join("database"); fs::create_dir(&history).unwrap(); fs::create_dir(&database).unwrap();
    for (root,name) in [(&history,"history-backup.json"),(&database,"database-backup.json")] { fs::write(root.join(name),b"{}").unwrap(); resize(&root.join(name),1<<30); }
    match restore_history_database(&history,dir.path().join("h-dest")).unwrap_err() { HistoryBackupError::Store(e)=>resource(e),e=>panic!("{e}") }
    match restore_database(&database,dir.path().join("d-dest")).unwrap_err() { DatabaseBackupError::Storage(e)=>resource(e),e=>panic!("{e}") }
    assert!(!dir.path().join("h-dest").exists()); assert!(!dir.path().join("d-dest").exists());
    assert!(!dir.path().join(".h-dest.bradian-g24-incomplete").exists()); assert!(!dir.path().join(".d-dest.bradian-g19-incomplete").exists());
}

#[test]
fn malformed_json_below_limit_keeps_corruption_classification_and_io_is_preserved() {
    let dir=tempfile::tempdir().unwrap(); fs::write(dir.path().join("history-backup.json"),b"{}").unwrap(); fs::write(dir.path().join("database-backup.json"),b"{}").unwrap();
    assert!(matches!(restore_history_database(dir.path(),dir.path().join("h")),Err(HistoryBackupError::Corruption(_))));
    assert!(matches!(restore_database(dir.path(),dir.path().join("d")),Err(DatabaseBackupError::Corruption(_))));
    let empty=dir.path().join("empty"); fs::create_dir(&empty).unwrap();
    assert!(matches!(RowStore::restore_backup(&empty,dir.path().join("native")),Err(StoreError::Io(_))));
    assert!(matches!(restore_history_database(&empty,dir.path().join("missing-history")),Err(HistoryBackupError::Io(_))));
    assert!(matches!(restore_database(&empty,dir.path().join("missing-database")),Err(DatabaseBackupError::Io(_))));
}
