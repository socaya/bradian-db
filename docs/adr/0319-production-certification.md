# ADR 0319 — External reproducibility + production certification

## Status

Accepted. Replaces the 2026-11-24 skeleton with a **real
end-to-end reproducibility check** for the lock-file axis.

## Context

Gate 9 requires a certification package binding every
shipped artefact to its reproducible build inputs. A
customer handed a Bradian binary + manifest must be able
to answer, mechanically: "if I rebuild from this commit
SHA with this rustc on this lockfile, do I get a
byte-identical artefact?"

The first mechanical check that can be shipped today is
the **lockfile axis**: BLAKE3-hash the exact `Cargo.lock`
bytes that pinned every dependency version and transitive
version, and verify the hash on reload. Commit SHA and
rustc version are build-time inputs the certifying tool
supplies.

## Decision

`bradian_core::certification::CertManifest`:

```rust
pub struct CertManifest {
    pub commit_sha:      String,   // 40-char hex, SHA-1
    pub cargo_lock_hash: String,   // 64-char hex, BLAKE3-256
    pub rustc_version:   String,
    pub verified_tests:  Vec<String>,
    pub signature:       Option<String>,
}

impl CertManifest {
    pub fn from_build(commit_sha, rustc_version, verified_tests, cargo_lock_path)
        -> CertResult<Self>;
    pub fn validate_shape(&self)                           -> CertResult<()>;
    pub fn verify_against(&self, cargo_lock_path: &Path)   -> CertResult<()>;
    pub fn to_json  (&self)         -> String;
    pub fn from_json(bytes: &[u8])  -> CertResult<Self>;
}
```

- `from_build` stream-hashes the lockfile in 64 KiB chunks
  into BLAKE3-256 and populates the manifest's
  `cargo_lock_hash`.
- `verify_against` re-reads the lockfile at reload time
  and refuses with `LockDrift` if the recomputed hash
  does not constant-time-match the manifest value.
- `signature` is reserved for a downstream ADR that
  chooses an algorithm and key material; `None` means
  "unsigned manifest" and the reproducibility check still
  runs — it just isn't provenance-gated.

## Scope

**In scope for this slice:**
- Real BLAKE3-256 hashing of a `Cargo.lock` on disk, with
  bounded-memory streaming (64 KiB chunks).
- Shape validation (hex length + alphabet).
- Constant-time hash comparison via the `subtle` crate.
- JSON round-trip (pretty-printed, deterministic field
  order via `serde` derive).
- End-to-end test against the live workspace
  `Cargo.lock` found by walking up from
  `CARGO_MANIFEST_DIR`: `from_build` → `verify_against`
  succeeds.
- Drift test: mutate the file by one byte and confirm
  `LockDrift` is surfaced.
- Missing-file test: surface `CertError::Io` with the
  underlying context.

**Out of scope** (dedicated follow-ups):
- Signature algorithm + key material (ADR 0326 CI infra).
- Reproducible-build verification across rustc versions
  and target triples.
- Supply-chain audit (crate source-tree checksums).

## Consequences

- An operator or auditor can take a shipped manifest,
  pair it with the repo's `Cargo.lock` at the recorded
  commit SHA, and mechanically verify the dep closure
  hasn't drifted.
- The `subtle` crate is now a direct dep of
  `bradian-core` (it was already in `bradian-relational`
  for a similar reason; no new transitive deps).
- 8 tests total: 7 unit tests cover hash parity against
  the in-memory BLAKE3 reference, build-populates-hash,
  verify-matches, single-byte drift detection, JSON
  round-trip with signature field, shape rejection at
  load, missing-file IO surfacing; 1 integration test
  runs the whole flow against the live workspace
  `Cargo.lock`.
- The 2026-11-24 "SKELETON" status on 0319 is retired.
