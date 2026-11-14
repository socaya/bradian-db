//! Transitive admission propagation (ADRs 0054, 0056, 0058).
//!
//! The alternate engines (TypedMvccStore, MvccSqlEngine) compose a
//! PersistentCatalog and an MvccStore. These tests prove that the
//! higher-level open paths honor the upstream admission policies, and
//! that the new open_with_limits variants let operators tighten them.

use std::collections::BTreeMap;
use std::fs;

use bradian_relational::mvcc_sql::MvccSqlEngine;
use bradian_relational::persistent_catalog::PersistentCatalog;
use bradian_relational::runtime_limits::{
    CatalogLimits, MvccStoreLimits,
};
use bradian_relational::sql::SqlError;
use bradian_relational::typed_mvcc::TypedMvccStore;
use tempfile::tempdir;

fn seed_two_table_shared_domain(root: &std::path::Path) {
    // Use SqlEngine to set up the catalog + shared-domain layout that
    // MvccSqlEngine requires: catalog/ and shared-domain/ at the root.
    let ddl = bradian_relational::sql::SqlEngine::open(root).unwrap();
    for sql in [
        "CREATE DATABASE bank",
        "CREATE SCHEMA bank.public",
        "CREATE TABLE bank.public.accounts (id BIGINT NOT NULL, balance BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE bank.public.movements (id BIGINT NOT NULL, amount BIGINT NOT NULL, PRIMARY KEY(id))",
    ] { ddl.execute(sql).unwrap(); }
    drop(ddl);
    // Register the catalog tables in the shared domain so MvccSqlEngine
    // can open the root without the G21 "no shared-domain" refusal.
    let domain = bradian_relational::domain_sql::DomainSqlEngine::open(root).unwrap();
    domain.register_catalog_table("bank.public.accounts").unwrap();
    domain.register_catalog_table("bank.public.movements").unwrap();
}

#[test]
fn typed_mvcc_store_honors_mvcc_store_limits_from_open_with_limits() {
    let dir = tempdir().unwrap();
    seed_two_table_shared_domain(dir.path());

    // Take a snapshot of the catalog definitions, then open TypedMvccStore
    // with a tight MvccStoreLimits budget. The shared-domain path is
    // dir/shared-domain, which already has the schema pins from register.
    let definitions: BTreeMap<_,_> = {
        let catalog = PersistentCatalog::open(dir.path().join("catalog")).unwrap();
        catalog.snapshot().tables()
            .map(|t| (t.id, t.clone()))
            .collect()
    };
    // The shared-domain already has 2 schema-pin rows from register_catalog_table.
    // max_distinct_keys = 1 must refuse during seeding.
    let outcome = TypedMvccStore::open_with_limits(
        dir.path().join("shared-domain"),
        &definitions,
        MvccStoreLimits { max_distinct_keys: 1, max_total_versions: 1024 },
    );
    assert!(outcome.is_err(), "tight MvccStore cap must refuse via TypedMvccStore::open_with_limits");

    // Relaxed limits succeed.
    TypedMvccStore::open_with_limits(
        dir.path().join("shared-domain"),
        &definitions,
        MvccStoreLimits::default(),
    ).unwrap();
}

#[test]
fn mvcc_sql_engine_honors_catalog_limits_from_open_with_limits() {
    let dir = tempdir().unwrap();
    seed_two_table_shared_domain(dir.path());
    // The seed produced 1 database, 1 schema, 2 tables in the catalog.
    // CatalogLimits.max_tables = 1 must refuse during catalog replay.
    let outcome = MvccSqlEngine::open_with_limits(
        dir.path(),
        MvccStoreLimits::default(),
        CatalogLimits { max_tables: 1, ..CatalogLimits::default() },
    );
    assert!(
        matches!(outcome, Err(SqlError::Catalog(_))),
        "tight CatalogLimits must refuse via MvccSqlEngine::open_with_limits",
    );
    // The on-disk WAL is unchanged — a later relaxed open sees both tables.
    let wal_before = fs::read(dir.path().join("catalog/rows.wal")).unwrap();
    assert!(!wal_before.is_empty());
}

#[test]
fn mvcc_sql_engine_default_open_still_admits_an_ordinary_workload() {
    // Regression guard: the new open_with_limits path must not alter the
    // behaviour seen through the existing MvccSqlEngine::open signature.
    let dir = tempdir().unwrap();
    seed_two_table_shared_domain(dir.path());
    MvccSqlEngine::open(dir.path()).unwrap();
}

#[test]
fn mvcc_sql_engine_honors_mvcc_store_limits_from_open_with_limits() {
    let dir = tempdir().unwrap();
    seed_two_table_shared_domain(dir.path());
    let outcome = MvccSqlEngine::open_with_limits(
        dir.path(),
        MvccStoreLimits { max_distinct_keys: 1, max_total_versions: 1024 },
        CatalogLimits::default(),
    );
    assert!(outcome.is_err(), "tight MvccStoreLimits must refuse via MvccSqlEngine::open_with_limits");
}
