use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::history_migration::migrate_shared_to_history;
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

pub fn migrated() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    let history = dir.path().join("history");
    {
        let ddl = SqlEngine::open(&source).unwrap();
        for statement in [
            "CREATE DATABASE bank", "CREATE SCHEMA bank.public",
            "CREATE TABLE bank.public.accounts (id BIGINT NOT NULL, balance BIGINT NOT NULL, PRIMARY KEY(id))",
            "CREATE TABLE bank.public.audit (id BIGINT NOT NULL, balance BIGINT NOT NULL, PRIMARY KEY(id))",
            "CREATE TABLE bank.public.messages (id BIGINT NOT NULL, body TEXT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(statement).unwrap(); }
    }
    {
        let db = DomainSqlEngine::open(&source).unwrap();
        for name in ["accounts", "audit", "messages"] {
            db.register_catalog_table(&format!("bank.public.{name}")).unwrap();
        }
        db.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute("INSERT INTO bank.public.accounts(id,balance) VALUES(1,100)")?;
            s.execute("INSERT INTO bank.public.accounts(id,balance) VALUES(2,200)")?;
            s.execute("INSERT INTO bank.public.audit(id,balance) VALUES(9,90)")?;
            s.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
    migrate_shared_to_history(&source, &history).unwrap();
    (dir, history)
}

pub fn query(db: &HistorySqlEngine, sql: &str) -> Vec<Vec<Value>> {
    db.with_session(|s| {
        s.execute("BEGIN")?;
        let result = s.execute(sql)?;
        s.execute("ROLLBACK")?;
        match result {
            SqlResultSet::Rows { rows, .. } => Ok(rows),
            other => panic!("expected rows: {other:?}"),
        }
    }).unwrap()
}

pub fn modify(db: &HistorySqlEngine, statements: &[&str]) -> u64 {
    db.with_session(|s| {
        s.execute("BEGIN")?;
        for statement in statements { s.execute(statement)?; }
        s.execute("COMMIT")?;
        Ok(s.last_commit_lsn().unwrap())
    }).unwrap()
}
