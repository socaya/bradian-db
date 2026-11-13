//! Run crash subprocesses in isolation so a fork cannot inherit other fixtures' locks.
#[path = "support/epoch_fixture.rs"]
mod fixture;
use fixture::{migrated, modify, query};
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::typed_table::Value;

fn row(value: i64) -> Vec<Value> { vec![Value::Int64(value)] }

#[test]
fn epoch_sql_recovers_after_process_exit_without_destructors() {
    const CHILD_ROOT: &str = "BRADIAN_EPOCH_SQL_CRASH_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let db = HistorySqlEngine::open(root).unwrap();
        db.enable_epoch_row_writes().unwrap();
        modify(&db, &["UPDATE bank.public.accounts SET balance=777 WHERE id=1"]);
        std::process::exit(77);
    }
    let (_dir, history) = migrated();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "epoch_sql_recovers_after_process_exit_without_destructors"])
        .env(CHILD_ROOT, &history).status().unwrap();
    assert_eq!(status.code(), Some(77));
    let db = HistorySqlEngine::open(&history).unwrap();
    assert!(db.epoch_row_writes_enabled().unwrap());
    assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![row(777)]);
}
