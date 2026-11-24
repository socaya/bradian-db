//! ADR 0319 — reproducibility check against the real
//! workspace `Cargo.lock`. The test walks up from
//! `CARGO_MANIFEST_DIR` to find the lock file, hashes it,
//! round-trips the manifest, and then re-verifies against
//! the same bytes.
//!
//! If someone edits `Cargo.lock` between `from_build` and
//! `verify_against` the test would detect it — which is
//! exactly the drift the certification check is for.

use std::path::{Path, PathBuf};

use bradian_core::certification::CertManifest;

fn find_workspace_lockfile() -> PathBuf {
    // CARGO_MANIFEST_DIR is `.../crates/bradian-core`. The
    // workspace lock is two levels up.
    let start = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut cur: &Path = start;
    loop {
        let candidate = cur.join("Cargo.lock");
        if candidate.is_file() { return candidate; }
        cur = cur.parent().expect("walked above the filesystem root without finding Cargo.lock");
    }
}

#[test]
fn hashes_and_verifies_the_live_workspace_lockfile() {
    let lock = find_workspace_lockfile();
    let manifest = CertManifest::from_build(
        "0".repeat(40),                                  // dummy commit SHA
        "rustc test",                                     // dummy rustc version
        vec!["cargo test --workspace --locked".into()],
        &lock,
    ).expect("certification manifest build");
    // Hash is non-empty hex of exactly 64 chars.
    assert_eq!(manifest.cargo_lock_hash.len(), 64);
    assert!(manifest.cargo_lock_hash.chars().all(|c| c.is_ascii_hexdigit()));
    // Re-verify against the same file — must succeed.
    manifest.verify_against(&lock).expect("verify against live Cargo.lock");
}
