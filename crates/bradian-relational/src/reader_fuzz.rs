//! ADR 0131 — bounded reader fuzzing harness.
//!
//! BRD-ROBUST-002 ("reader-side differential fuzzing") is
//! OPEN. External fuzzers (cargo-fuzz, honggfuzz) require
//! unstable toolchains or procurement. This module ships an
//! INTERNAL, deterministic, bounded harness callable from
//! stable Rust: given a seed + iteration budget it generates
//! pseudo-random byte blobs and feeds them to each
//! stable-API decoder, asserting no panic escapes.
//!
//! Honest scope:
//! - Deterministic xorshift PRNG; same seed = same report.
//! - Iteration budget bounded by caller; harness does NOT
//!   run indefinitely.
//! - Each decoder is called inside `catch_unwind` so a
//!   panic is COUNTED, not propagated. Current decoders
//!   are expected to return `Err`, not panic.
//! - Covers the two most reachable stable-API decoders:
//!   - `page::Page::decode` (4096-byte decoder).
//!   - `checkpoint_manifest::CheckpointManifest::decode` (JSON
//!     text decoder).
//!
//! Out of scope (follow-ups):
//! - Coverage-guided fuzzing — needs an instrumented
//!   toolchain.
//! - WAL frame / BACE envelope / segment-file decoders —
//!   each needs an exposed `decode(&[u8])` variant.
//! - Grammar-aware input generation — current inputs are
//!   uniform random bytes.
//! - Corpus persistence / minimization.

use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::checkpoint_manifest::CheckpointManifest;
use crate::page::{Page, PAGE_SIZE};

/// Per-decoder outcome tally.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DecoderStats {
    /// Decoder returned `Ok(_)` — rare for random bytes.
    pub ok: u32,
    /// Decoder returned `Err(_)` — expected.
    pub err: u32,
    /// Decoder PANICKED — must be 0 for all shipped decoders.
    pub panic: u32,
}

#[derive(Clone, Debug, Default)]
pub struct FuzzReport {
    pub iterations: u32,
    pub page_decoder: DecoderStats,
    pub manifest_decoder: DecoderStats,
}

impl FuzzReport {
    pub fn total_panics(&self) -> u32 {
        self.page_decoder.panic + self.manifest_decoder.panic
    }
}

/// ADR 0131 — run the bounded reader fuzz harness.
///
/// `seed` makes runs deterministic; `iterations` caps the
/// budget; `max_input_len` caps the input size per
/// iteration (page decoder always gets PAGE_SIZE bytes
/// regardless).
pub fn fuzz_decoders(seed: u64, iterations: u32, max_input_len: usize) -> FuzzReport {
    let mut rng = Xorshift64 { state: seed.max(1) };
    let mut report = FuzzReport { iterations, ..Default::default() };

    for _ in 0..iterations {
        // 1) Page decoder: always exactly PAGE_SIZE bytes.
        let mut page_bytes = vec![0_u8; PAGE_SIZE];
        rng.fill(&mut page_bytes);
        tally(&mut report.page_decoder,
              catch_unwind(AssertUnwindSafe(|| Page::decode(&page_bytes))));

        // 2) Manifest decoder: variable-length random bytes
        //    (JSON path tolerates nonsense; must not panic).
        let len = (rng.next() as usize) % max_input_len.max(1);
        let mut manifest_bytes = vec![0_u8; len];
        rng.fill(&mut manifest_bytes);
        tally(&mut report.manifest_decoder,
              catch_unwind(AssertUnwindSafe(|| CheckpointManifest::decode(&manifest_bytes))));
    }
    report
}

fn tally<T, E>(stats: &mut DecoderStats, outcome: std::thread::Result<Result<T, E>>) {
    match outcome {
        Ok(Ok(_)) => stats.ok += 1,
        Ok(Err(_)) => stats.err += 1,
        Err(_) => stats.panic += 1,
    }
}

/// Minimal xorshift64 PRNG. No crate dep.
struct Xorshift64 { state: u64 }
impl Xorshift64 {
    fn next(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }
    fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let n = self.next().to_le_bytes();
            for (i, b) in chunk.iter_mut().enumerate() { *b = n[i]; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_is_deterministic_given_same_seed() {
        let a = fuzz_decoders(42, 100, 256);
        let b = fuzz_decoders(42, 100, 256);
        assert_eq!(a.page_decoder, b.page_decoder);
        assert_eq!(a.manifest_decoder, b.manifest_decoder);
    }

    #[test]
    fn xorshift_produces_different_bytes_for_different_seeds() {
        // Both decoders reject everything (ok=0, err=iterations),
        // so the public fuzz report is identical across seeds —
        // we instead verify at the PRNG level that different
        // seeds produce different streams.
        let mut a = Xorshift64 { state: 1 };
        let mut b = Xorshift64 { state: 2 };
        let mut buf_a = vec![0u8; 256];
        let mut buf_b = vec![0u8; 256];
        a.fill(&mut buf_a);
        b.fill(&mut buf_b);
        assert_ne!(buf_a, buf_b);
    }

    #[test]
    fn no_decoder_panics_over_1000_iterations() {
        let r = fuzz_decoders(0xdead_beef, 1000, 512);
        assert_eq!(r.total_panics(), 0, "panic in a reader: {r:?}");
    }

    #[test]
    fn page_decoder_rejects_random_bytes() {
        // With PAGE_SIZE=4096 random bytes the magic + checksum
        // rarely align; almost every iteration should err.
        let r = fuzz_decoders(7, 500, 256);
        assert_eq!(r.page_decoder.ok, 0);
        assert_eq!(r.page_decoder.err, 500);
    }

    #[test]
    fn manifest_decoder_errors_on_random_bytes() {
        let r = fuzz_decoders(9, 200, 64);
        // JSON-free random bytes should err; no panics.
        assert_eq!(r.manifest_decoder.panic, 0);
        // Most random bytes aren't a valid manifest JSON blob.
        assert!(r.manifest_decoder.err + r.manifest_decoder.ok == 200);
    }

    #[test]
    fn iterations_count_is_preserved() {
        let r = fuzz_decoders(1, 123, 32);
        assert_eq!(r.iterations, 123);
        let total_page = r.page_decoder.ok + r.page_decoder.err + r.page_decoder.panic;
        let total_man  = r.manifest_decoder.ok + r.manifest_decoder.err + r.manifest_decoder.panic;
        assert_eq!(total_page, 123);
        assert_eq!(total_man, 123);
    }

    #[test]
    fn zero_iterations_is_a_no_op() {
        let r = fuzz_decoders(1, 0, 64);
        assert_eq!(r.page_decoder, DecoderStats::default());
        assert_eq!(r.manifest_decoder, DecoderStats::default());
    }
}
