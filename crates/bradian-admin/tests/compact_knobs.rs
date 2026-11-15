//! ADR 0085 — CLI exposure of column-dict + multi-page knobs.
//!
//! Verifies the `--max-rows-per-page` and `--use-column-dict`
//! flags exist on `bradian-db compact-partition` + `bradian-db
//! export-snapshot`, and that `--max-rows-per-page 0` is
//! rejected.
use std::process::Command;

static PROCESS_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn binary() -> &'static str { env!("CARGO_BIN_EXE_bradian-db") }

// ---------- §3.4 item 7 — --help documents both flags ----------

#[test]
fn item_7_compact_help_documents_new_flags() {
    let _serial = PROCESS_TESTS.lock().unwrap();
    let out = Command::new(binary()).args(["compact-partition", "--help"])
        .output().unwrap();
    assert!(out.status.success());
    let help = String::from_utf8(out.stdout).unwrap();
    assert!(help.contains("--max-rows-per-page"), "help missing flag; got:\n{help}");
    assert!(help.contains("--use-column-dict"), "help missing flag; got:\n{help}");
    // ADR references should appear in the help text.
    assert!(help.contains("ADR 0083") || help.contains("ADR 0081"),
        "help missing ADR references; got:\n{help}");
}

#[test]
fn item_7b_export_help_documents_new_flags() {
    let _serial = PROCESS_TESTS.lock().unwrap();
    let out = Command::new(binary()).args(["export-snapshot", "--help"])
        .output().unwrap();
    assert!(out.status.success());
    let help = String::from_utf8(out.stdout).unwrap();
    assert!(help.contains("--max-rows-per-page"), "help missing flag; got:\n{help}");
    assert!(help.contains("--use-column-dict"), "help missing flag; got:\n{help}");
}

// ---------- §3.4 item 6 — --max-rows-per-page 0 rejected ----------

#[test]
fn item_6_zero_rows_per_page_is_rejected() {
    let _serial = PROCESS_TESTS.lock().unwrap();
    // Provide the other required args with harmless values; --max-rows-per-page 0
    // must fail before anything else runs.
    let out = Command::new(binary()).args([
        "compact-partition",
        "--root", "/tmp/nonexistent-root-for-rejection-test",
        "--dataset", "d",
        "--partition", "p",
        "--primary-key", "id",
        "--schema", "id:i64",
        "--max-rows-per-page", "0",
    ]).output().unwrap();
    assert!(!out.status.success(), "expected non-zero exit for --max-rows-per-page 0");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("must be > 0") || stderr.contains("max-rows-per-page"),
        "expected rejection message; got:\n{stderr}");
}
