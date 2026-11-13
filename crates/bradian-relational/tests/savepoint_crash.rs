//! Isolated process-exit qualification: no destructors before reopen.
#[path = "support/epoch_fixture.rs"]
mod fixture;
use fixture::{migrated, modify, query};
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::typed_table::Value;

#[test]
fn process_exit_keeps_only_outer_committed_work() {
    if let Some(root) = std::env::var_os("BRADIAN_SAVEPOINT_TEST_ROOT") {
        let db = HistorySqlEngine::open(root).unwrap();
        db.with_session(|s| -> bradian_relational::sql::SqlResult<()> {
            s.execute("BEGIN")?;
            s.execute("UPDATE bank.public.accounts SET balance=180 WHERE id=1")?;
            s.execute("SAVEPOINT before_discard")?;
            s.execute("UPDATE bank.public.audit SET balance=999 WHERE id=9")?;
            s.execute("ROLLBACK TO before_discard")?;
            s.execute("RELEASE before_discard")?;
            if std::env::var_os("BRADIAN_SAVEPOINT_TEST_COMMIT").is_some() {
                s.execute("COMMIT")?;
            }
            std::process::exit(77);
        }).unwrap();
        unreachable!();
    }
    for commit in [false, true] {
        let (_dir, root) = migrated();
        // Exercise the shared fixture writer here too; keeps its helper used in
        // this isolated binary and seeds a known durable pre-exit baseline.
        let db = HistorySqlEngine::open(&root).unwrap();
        modify(&db, &["UPDATE bank.public.accounts SET balance=100 WHERE id=1"]);
        let before = db.last_lsn();
        drop(db);
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "process_exit_keeps_only_outer_committed_work", "--nocapture"])
            .env("BRADIAN_SAVEPOINT_TEST_ROOT", &root)
            .env_remove("BRADIAN_SAVEPOINT_TEST_COMMIT");
        if commit { command.env("BRADIAN_SAVEPOINT_TEST_COMMIT", "1"); }
        let status = command.status().unwrap();
        assert_eq!(status.code(), Some(77));
        let db = HistorySqlEngine::open(&root).unwrap();
        assert_eq!(db.last_lsn(), before + u64::from(commit));
        assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"),
            vec![vec![Value::Int64(if commit { 180 } else { 100 })]]);
        assert_eq!(query(&db, "SELECT balance FROM bank.public.audit WHERE id=9"), vec![vec![Value::Int64(90)]]);
    }
}
