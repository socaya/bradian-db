# ADR 0131 — Bounded reader fuzz harness (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch D D3 primitive.
D1 (unique secondary enforcement) shipped in ADR 0101, D2
(non-unique index helper) in ADR 0127, D4 (auto-PITR
wiring) remains follow-up.

## 1. Scope

New module `bradian_relational::reader_fuzz`:

- `fuzz_decoders(seed, iterations, max_input_len) -> FuzzReport`
  — deterministic, bounded harness that feeds pseudo-
  random byte blobs to stable-API decoders.
- Each decoder runs inside `catch_unwind` so a panic is
  counted, not propagated.
- Two decoders covered:
  - `page::Page::decode` — 4096-byte decoder.
  - `checkpoint_manifest::CheckpointManifest::decode` —
    JSON text decoder.
- `DecoderStats { ok, err, panic }` and `FuzzReport { iterations,
  page_decoder, manifest_decoder }`.
- `Xorshift64` PRNG — stable, no crate dep; same seed =
  same stream.

## 2. Out of scope (follow-ups)

- **Coverage-guided fuzzing** — cargo-fuzz / honggfuzz
  requires instrumented toolchain + procurement (CI
  runner, corpus storage).
- **Additional decoders** — WAL frame, BACE envelope,
  segment file (.brcs), backup receipt, catalog
  snapshot, incremental backup delta all need an
  exposed `decode(&[u8])` signature before they can be
  added to the harness.
- **Grammar-aware input generation** — current inputs
  are uniform random bytes. Realistic manifest JSON
  would exercise more parse branches.
- **Corpus persistence and minimization** — findings
  (if any) are counts only. A failing seed isn't
  persisted; callers must capture the seed.
- **Decoder invariant checks beyond no-panic** — e.g.,
  "a decoded page always encode-round-trips bit-exact."

## 3. Compliance

- Deterministic: same seed → same tallies.
- Different seeds produce different byte streams (verified
  at the PRNG level since both decoders reject every
  random blob).
- Over 1000 iterations across both decoders, total panic
  count is 0.
- Page decoder rejects every random 4096-byte input
  (ok=0, err=iterations).
- Manifest decoder produces no panics on random bytes
  (ok+err == iterations).
- Iterations field is preserved; zero-iteration call is a
  no-op (empty report).
- 7 new tests in `reader_fuzz::tests`.

## 4. Honest status

Foundation only. **BRD-ROBUST-002 reader-side differential
fuzzing** is PARTIAL: a stable-Rust deterministic harness
exists and demonstrates no-panic on two decoders, but
coverage-guided fuzzing and grammar-aware corpus
generation remain OPEN.
