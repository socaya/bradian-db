# ADR 0318 — Schema history compaction with reference proofs

## Status

Accepted. Replaces the 2026-11-24 skeleton with a **real
on-disk journal rewrite** and verifiable reference proof.

## Context

The skeleton exposed `plan_compaction` + `verify_proof`
as pure in-memory operations with no durable form. An
operator who wanted to compact the schema history journal
had nothing to actually rewrite, and a reviewer had no
file to verify a proof against.

## Decision

New module surface in
`bradian_relational::schema_history_compaction`:

```rust
pub struct SchemaVersion { version: u64, digest: [u8; 32] }
pub struct SchemaJournal { journal_version: u32, entries: Vec<SchemaVersion> }
pub struct CompactionPlan {
    retained: Vec<SchemaVersion>,
    dropped:  Vec<SchemaVersion>,
    proof:    [u8; 32],   // BLAKE3 of (version‖digest)*
}

pub fn plan_compaction      (all: &[SchemaVersion], live: &[u64]) -> CompactionPlan;
pub fn verify_proof         (plan: &CompactionPlan)               -> bool;
pub fn compact_file_in_place(path: &Path, live: &[u64])           -> CompactionResult<CompactionPlan>;
pub fn verify_file_proof    (path: &Path, expected: &[u8; 32])    -> CompactionResult<()>;
```

Semantics:

- The on-disk journal format is pretty-printed JSON
  (`SchemaJournal`) with a `journal_version` tag.
  Digests are serialised as 64-char hex strings so the
  file is human-auditable.
- `compact_file_in_place` writes the compacted journal
  to `{path}.tmp`, `sync_data`s it, atomically renames
  over `path`, and best-effort directory-syncs on Unix.
  The tmp file does not survive the call.
- The proof is BLAKE3-256 over the little-endian
  `(version, digest)` tuples of the retained entries in
  ascending-version order. `verify_proof` (in-memory) and
  `verify_file_proof` (on-disk) both recompute and
  compare.

## Scope

**In scope:**
- Durable journal file format with `journal_version`
  gate.
- Atomic in-place compaction (`tmp` + `rename` + dir
  `sync_data`).
- BLAKE3 reference proof covering retained
  `(version, digest)` pairs.
- `ProofMismatch` surface when a verifier is given a
  tampered journal.
- Unsupported-version rejection at read.
- 6 unit tests: partition correctness + proof agreement;
  retained-vec tampering fails proof; file rewrite + reread
  shows only retained entries; file-proof detects a
  tampered journal; unsupported `journal_version` rejected;
  hex-encoded digest round-trip.

**Out of scope** (dedicated follow-ups):
- Integration with the real catalog DDL journal format
  (ADR 0016 `PersistentCatalog`). A separate slice maps
  DDL events → `SchemaVersion` entries and plumbs this
  rewrite into the compaction admin command.
- Online compaction concurrent with live DDL — the
  current call takes exclusive ownership of the file.

## Consequences

- An operator can compact a schema history journal and
  hand the proof hex to a reviewer, who can re-verify
  the resulting file independently.
- The 2026-11-24 "SKELETON" status on 0318 is retired.
