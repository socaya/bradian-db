//! G18 recovery tests: lifecycle intents and domain pin reconciliation.

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::lifecycle::{CatalogLifecycle, LifecycleError, TableLifecycleState};
use bradian_relational::persistent_catalog::PersistentCatalog;
use bradian_relational::row_store::RowStore;
use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::{Table, TableId};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn setup(root: &std::path::Path) -> (Table, Table) {
    {
        let sql = SqlEngine::open(root).unwrap();
        for statement in [
            "CREATE DATABASE billing",
            "CREATE SCHEMA billing.public",
            "CREATE TABLE billing.public.invoices (id BIGINT NOT NULL, amount BIGINT NOT NULL, PRIMARY KEY(id))",
            "CREATE TABLE billing.public.payments (id BIGINT NOT NULL, invoice_id BIGINT NOT NULL, PRIMARY KEY(id))",
        ] {
            assert_eq!(sql.execute(statement).unwrap(), SqlResultSet::Command { affected: 0 });
        }
    }
    let cat = PersistentCatalog::open(root.join("catalog")).unwrap();
    let view = cat.snapshot();
    let invoices = view.tables().find(|table| table.name == "invoices").unwrap().clone();
    let payments = view.tables().find(|table| table.name == "payments").unwrap().clone();
    (invoices, payments)
}

fn force_pending(root: &std::path::Path, table: &Table) {
    let key = format!("table/{:016x}", table.id.0);
    let raw = RowStore::open(root.join("lifecycle")).unwrap();
    let existing: serde_json::Value = serde_json::from_slice(&raw.get(key.as_bytes()).unwrap()).unwrap();
    let mut changed = existing;
    changed["state"] = serde_json::Value::String("Pending".into());
    raw.put(key.as_bytes(), &serde_json::to_vec(&changed).unwrap()).unwrap();
}

#[test]
fn provision_catalog_tables_and_run_cross_table_sql_after_restart() {
    let dir = tempdir().unwrap();
    let (invoices, payments) = setup(dir.path());
    {
        let owner = CatalogLifecycle::open(dir.path()).unwrap();
        assert_eq!(owner.state(invoices.id).unwrap(), None);
        assert_eq!(owner.state(payments.id).unwrap(), None);
        owner.provision_existing(invoices.id).unwrap();
        owner.provision_existing(payments.id).unwrap();
        assert_eq!(owner.state(invoices.id).unwrap(), Some(TableLifecycleState::Active));
        assert_eq!(owner.reconcile().unwrap().len(), 2);
    }
    {
        let sql = DomainSqlEngine::open(dir.path()).unwrap();
        sql.with_session(|session| {
            session.execute("BEGIN")?;
            session.execute("INSERT INTO billing.public.invoices(id,amount) VALUES(1,800)")?;
            session.execute("INSERT INTO billing.public.payments(id,invoice_id) VALUES(9,1)")?;
            session.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
    let reopened = CatalogLifecycle::open(dir.path()).unwrap();
    assert_eq!(reopened.reconcile().unwrap().len(), 2);
    assert_eq!(reopened.state(payments.id).unwrap(), Some(TableLifecycleState::Active));
    drop(reopened);
    let sql = DomainSqlEngine::open(dir.path()).unwrap();
    sql.with_session(|session| {
        session.execute("BEGIN")?;
        assert_eq!(
            session.execute("SELECT amount FROM billing.public.invoices WHERE id=1")?,
            SqlResultSet::Rows {
                columns: vec!["amount".into()],
                rows: vec![vec![Value::Int64(800)]],
            });
        assert_eq!(
            session.execute("SELECT invoice_id FROM billing.public.payments WHERE id=9")?,
            SqlResultSet::Rows {
                columns: vec!["invoice_id".into()],
                rows: vec![vec![Value::Int64(1)]],
            });
        session.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn interrupted_pending_intent_recovers_idempotently_when_pin_already_exists() {
    let dir = tempdir().unwrap();
    let (table, _) = setup(dir.path());
    {
        let coordinator = CatalogLifecycle::open(dir.path()).unwrap();
        coordinator.provision_existing(table.id).unwrap();
    }
    force_pending(dir.path(), &table);
    let recovered = CatalogLifecycle::open(dir.path()).unwrap();
    assert_eq!(recovered.state(table.id).unwrap(), Some(TableLifecycleState::Active));
    assert_eq!(recovered.reconcile().unwrap().len(), 1);
    assert_eq!(recovered.provision_existing(table.id).unwrap(), recovered.last_domain_lsn());
}

#[test]
fn pending_intent_before_schema_pin_is_replayed_on_open() {
    let dir = tempdir().unwrap();
    let (table, _) = setup(dir.path());
    {
        let owner = CatalogLifecycle::open(dir.path()).unwrap();
        assert_eq!(owner.state(table.id).unwrap(), None);
    }
    {
        let raw = RowStore::open(dir.path().join("lifecycle")).unwrap();
        let entry = serde_json::json!({
            "version": 1,
            "table": table.clone(),
            "state": "Pending",
        });
        raw.put(
            format!("table/{:016x}", table.id.0).as_bytes(),
            &serde_json::to_vec(&entry).unwrap(),
        ).unwrap();
    }
    let recovered = CatalogLifecycle::open(dir.path()).unwrap();
    assert_eq!(recovered.state(table.id).unwrap(), Some(TableLifecycleState::Active));
    drop(recovered);
    let sql = DomainSqlEngine::open(dir.path()).unwrap();
    sql.with_session(|session| {
        session.execute("BEGIN")?;
        session.execute("INSERT INTO billing.public.invoices(id,amount) VALUES(2,100)")?;
        session.execute("COMMIT")?;
        Ok(())
    }).unwrap();
}

#[test]
fn bootstrap_existing_g16_pins_without_rewriting_rows() {
    let dir = tempdir().unwrap();
    let (table, _) = setup(dir.path());
    {
        let sql = DomainSqlEngine::open(dir.path()).unwrap();
        sql.register_catalog_table("billing.public.invoices").unwrap();
        sql.with_session(|session| {
            session.execute("BEGIN")?;
            session.execute("INSERT INTO billing.public.invoices(id,amount) VALUES(7,70)")?;
            session.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
    let owner = CatalogLifecycle::open(dir.path()).unwrap();
    assert_eq!(owner.state(table.id).unwrap(), Some(TableLifecycleState::Active));
    drop(owner);
    let sql = DomainSqlEngine::open(dir.path()).unwrap();
    sql.with_session(|session| {
        session.execute("BEGIN")?;
        assert_eq!(session.execute("SELECT amount FROM billing.public.invoices WHERE id=7")?,
            SqlResultSet::Rows {
                columns: vec!["amount".into()],
                rows: vec![vec![Value::Int64(70)]],
            });
        Ok(())
    }).unwrap();
}

#[test]
fn unknown_catalog_identity_is_not_journaled() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let owner = CatalogLifecycle::open(dir.path()).unwrap();
    let original = owner.last_lifecycle_lsn();
    assert!(matches!(owner.provision_existing(TableId(999999)), Err(LifecycleError::MissingCatalogTable(_))));
    assert_eq!(owner.last_lifecycle_lsn(), original);
}

#[test]
fn orphan_domain_pin_without_matching_catalog_fails_closed() {
    let dir = tempdir().unwrap();
    let (invoice, _) = setup(dir.path());
    let mut orphan = invoice.clone();
    orphan.id = TableId(invoice.id.0 + 10000);
    {
        use bradian_relational::transaction_domain::MultiTableStore;
        let domain = MultiTableStore::open(dir.path().join("shared-domain")).unwrap();
        domain.register_table(orphan).unwrap();
    }
    assert!(matches!(CatalogLifecycle::open(dir.path()), Err(LifecycleError::Integrity(_))));
}

#[test]
fn corrupted_or_unrecognized_lifecycle_marker_refuses_open() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    {
        let owner = CatalogLifecycle::open(dir.path()).unwrap();
        assert!(owner.last_lifecycle_lsn() >= 1);
    }
    {
        let raw = RowStore::open(dir.path().join("lifecycle")).unwrap();
        raw.put(b"bradian.lifecycle.format", b"BAD-MARKER").unwrap();
    }
    assert!(matches!(CatalogLifecycle::open(dir.path()), Err(LifecycleError::Integrity(_))));
}

#[test]
fn lifecycle_rejects_unknown_journal_key_without_mutating_domain() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    {
        let owner = CatalogLifecycle::open(dir.path()).unwrap();
        assert_eq!(owner.last_domain_lsn(), 0);
    }
    {
        let raw = RowStore::open(dir.path().join("lifecycle")).unwrap();
        raw.put(b"other-key", b"malformed").unwrap();
    }
    assert!(matches!(CatalogLifecycle::open(dir.path()), Err(LifecycleError::Integrity(_))));
}

#[test]
fn active_pin_missing_from_domain_is_an_integrity_failure() {
    let dir = tempdir().unwrap();
    let (table, _) = setup(dir.path());
    {
        let owner = CatalogLifecycle::open(dir.path()).unwrap();
        owner.provision_existing(table.id).unwrap();
    }
    // A production controller must not silently accept one side of a
    // previously ACTIVE registration being lost.
    std::fs::remove_dir_all(dir.path().join("shared-domain")).unwrap();
    assert!(matches!(CatalogLifecycle::open(dir.path()), Err(LifecycleError::Integrity(_))));
}

#[test]
fn legacy_table_files_block_lifecycle_coordinator() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    std::fs::create_dir_all(dir.path().join("tables").join("42")).unwrap();
    assert!(matches!(CatalogLifecycle::open(dir.path()), Err(LifecycleError::Integrity(_))));
}

#[test]
fn active_coordinator_prevents_competing_catalog_and_domain_owners() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let owner = CatalogLifecycle::open(dir.path()).unwrap();
    assert!(PersistentCatalog::open(dir.path().join("catalog")).is_err());
    assert!(CatalogLifecycle::open(dir.path()).is_err());
    drop(owner);
    assert!(CatalogLifecycle::open(dir.path()).is_ok());
}
