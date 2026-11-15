//! ADR 0074 — closes the scope-limit deferrals from
//! ADRs 0070 §6 item 6, 0071 §3.5 items 4–5, and 0072 §3.5
//! items 4–5. Those ADRs noted that the "fresh-open +
//! idempotent-checkpoint rule" (RowStore::checkpoint returns
//! early when inner.checkpoint_lsn == inner.lsn) prevents a
//! fresh BACE-opened engine from emitting v2 pages without an
//! intervening mutation.
//!
//! The G23 DDL coordinator (`HistorySqlEngine::execute_ddl` +
//! `enable_epoch_row_writes`) is the natural mutation path:
//! it advances both the catalog LSN and the history-domain LSN
//! through its activation protocol, so a subsequent
//! `checkpoint()` emits v2 pages under BACE wiring per ADR 0065.
//!
//! This file is test-only. No production code changed for this
//! ADR.

#[path = "support/epoch_fixture.rs"]
#[allow(dead_code)]
mod fixture;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use fixture::{migrated, query};

use bradian_relational::bace::codec::raw::RAW_ID;
use bradian_relational::bace::codec::zstd::ZSTD_ID;
use bradian_relational::bace::envelope::ENVELOPE_MAGIC;
use bradian_relational::bace::policy::{
    CompressionProfile, InMemoryPolicyService, PlatformBaseline,
};
use bradian_relational::bace::registry::BuiltInRegistry;
use bradian_relational::history_backup::restore_history_database_with_bace;
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::row_store::BaceWiring;

fn wiring_zstd_balanced() -> BaceWiring {
    let baseline = PlatformBaseline {
        profile: CompressionProfile::Balanced,
        allowed_codecs: vec![ZSTD_ID, RAW_ID],
        ..PlatformBaseline::shipped()
    };
    BaceWiring {
        policy_service: Arc::new(InMemoryPolicyService::new(baseline, BuiltInRegistry::new())),
        registry: Arc::new(BuiltInRegistry::new()),
        authentication: None,
    }
}

/// Open the migrated G24 root under BACE wiring, run a bounded
/// `execute_ddl` ALTER, and force a checkpoint. On return, the
/// `root/catalog/` and `root/history-domain/` directories hold
/// snapshot files produced by the BACE-aware checkpoint writer.
fn force_v2_pages_via_ddl(root: &Path, wiring: &BaceWiring) {
    let mut db = HistorySqlEngine::open_with_bace(root, wiring.clone()).unwrap();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD memo TEXT").unwrap();
    db.checkpoint().unwrap();
    // Dropping the engine releases its exclusive component locks
    // so the test can inspect the resulting files.
}

/// Walk every `snapshot-*.brp` file under `component_dir` and
/// collect each page's format_version plus a boolean indicating
/// whether the page starts with the BACE envelope magic at page
/// offset 32 (the v2 layout per ADR 0064).
fn snapshot_page_versions(component_dir: &Path) -> Vec<(u16, bool, u16)> {
    let mut rows = Vec::new();
    for entry in fs::read_dir(component_dir).unwrap().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy().to_string();
        if !(name.starts_with("snapshot-") && name.ends_with(".brp")) { continue; }
        let bytes = fs::read(entry.path()).unwrap();
        for page_idx in 0..(bytes.len() / 4096) {
            let base = page_idx * 4096;
            let version = u16::from_le_bytes([bytes[base + 4], bytes[base + 5]]);
            let envelope_matches = version == 2
                && bytes[base + 32..base + 36] == ENVELOPE_MAGIC;
            let codec_id = if envelope_matches {
                u16::from_le_bytes([bytes[base + 32 + 6], bytes[base + 32 + 7]])
            } else { 0 };
            rows.push((version, envelope_matches, codec_id));
        }
    }
    rows
}

#[test]
fn ddl_under_bace_wiring_emits_v2_pages_in_history_domain_snapshot() {
    // ADR 0074 §3.2 item 1 — demonstrates ZSTD-wrapped v2 pages
    // in the history-domain snapshot directly after
    // `HistorySqlEngine::checkpoint()`. The engine's checkpoint
    // entry point only drives the history-domain store (not the
    // catalog); the catalog analog is covered by test 5 which
    // uses `backup_to_with_bace` whose catalog sub-step
    // triggers an own catalog checkpoint.
    let (_dir, root) = migrated();
    let wiring = wiring_zstd_balanced();
    force_v2_pages_via_ddl(&root, &wiring);

    let rows = snapshot_page_versions(&root.join("history-domain"));
    assert!(!rows.is_empty(), "history-domain: expected snapshot pages");
    let v2 = rows.iter().filter(|(v, _, _)| *v == 2).count();
    assert!(
        v2 > 0,
        "history-domain: expected at least one v2 page, saw {rows:?}",
    );
    let zstd_pages = rows.iter()
        .filter(|(v, env, codec)| *v == 2 && *env && *codec == ZSTD_ID.0)
        .count();
    assert!(
        zstd_pages > 0,
        "history-domain: expected at least one ZSTD-wrapped page, saw {rows:?}",
    );
}

#[test]
fn legacy_history_sql_engine_open_on_post_ddl_bace_root_fails_closed() {
    // ADR 0074 §3.2 item 3 — reaches the fail-closed path that
    // ADR 0070 §6 item 6 could not exercise. With actual v2
    // pages on disk, `HistorySqlEngine::open` must return a
    // BACE-origin error.
    let (_dir, root) = migrated();
    let wiring = wiring_zstd_balanced();
    force_v2_pages_via_ddl(&root, &wiring);

    let msg = match HistorySqlEngine::open(&root) {
        Err(e) => format!("{e}"),
        Ok(_) => panic!("expected legacy open to fail on BACE-written root"),
    };
    assert!(
        msg.contains("open_with_bace") || msg.contains("BACE") || msg.contains("page_id"),
        "expected BACE-origin error surface, got: {msg}",
    );
}

#[test]
fn history_sql_engine_open_with_bace_after_ddl_reads_altered_schema() {
    // ADR 0074 §3.2 item 4 — reopening with the same wiring
    // succeeds and the ALTER-added column is readable (and
    // defaults to NULL for the pre-existing rows).
    let (_dir, root) = migrated();
    let wiring = wiring_zstd_balanced();
    force_v2_pages_via_ddl(&root, &wiring);

    let db = HistorySqlEngine::open_with_bace(&root, wiring).unwrap();
    let rows = query(&db, "SELECT balance, memo FROM bank.public.accounts WHERE id=1");
    assert_eq!(rows.len(), 1, "expected the pre-DDL row to survive the schema evolution");
    // The pre-DDL row had no memo column; projection after ALTER
    // yields the declared NULL default for existing rows.
    assert_eq!(
        rows[0].len(), 2,
        "projection must include the newly-added memo column",
    );
}

#[test]
fn bace_wrapped_post_ddl_backup_round_trips_through_restore_with_bace() {
    // ADR 0074 §3.2 item 5 — end-to-end: BACE backup produced
    // AFTER a DDL mutation carries v2 pages in its sub-archives,
    // the flag is set, and restore_history_database_with_bace
    // round-trips through them.
    let (dir, root) = migrated();
    let wiring = wiring_zstd_balanced();
    force_v2_pages_via_ddl(&root, &wiring);

    let archive = dir.path().join("archive-post-ddl");
    {
        let db = HistorySqlEngine::open_with_bace(&root, wiring.clone()).unwrap();
        db.backup_to_with_bace(&archive).unwrap();
    }

    // Sub-archive flag + v2 pages observable inside.
    for component in ["catalog", "history-domain"] {
        let receipt = bradian_relational::backup::read_receipt(
            &archive.join(component).join("backup.info"),
        ).unwrap();
        assert!(
            receipt.flags.payload_has_bace_pages,
            "{component} sub-archive must carry BACE flag",
        );
        let pages = snapshot_page_versions(&archive.join(component));
        let v2 = pages.iter().filter(|(v, _, _)| *v == 2).count();
        assert!(
            v2 > 0,
            "{component} sub-archive must contain v2 pages, saw {pages:?}",
        );
    }

    let restored = dir.path().join("restored-post-ddl");
    restore_history_database_with_bace(&archive, &restored, wiring.clone()).unwrap();

    let db = HistorySqlEngine::open_with_bace(&restored, wiring).unwrap();
    let rows = query(&db, "SELECT balance, memo FROM bank.public.accounts WHERE id=1");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 2);
}
