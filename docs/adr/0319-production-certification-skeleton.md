# ADR 0319 — External reproducibility + production certification: skeleton

## Status

Accepted as a skeleton. Twelfth slice of P2, closing P2.

## Context

Gate 9 requires a certification package: a reproducibility
manifest covering build inputs (commit SHA, Cargo.lock
digest, rustc version), a declared set of verified test
runs, and a signature placeholder for the certifying
party. A full certification protocol is beyond the code
scope of this slice.

## Decision

New module `bradian_core::certification` (lightweight; it
is referenced by every crate indirectly via `bradian-core`
already):

```rust
pub struct CertManifest {
    pub commit_sha:       String,   // 40-hex
    pub cargo_lock_hash:  String,   // 64-hex (BLAKE3)
    pub rustc_version:    String,
    pub verified_tests:   Vec<String>,
    pub signature_placeholder: Option<String>,
}
impl CertManifest {
    pub fn validate_shape(&self) -> CertResult<()>;
    pub fn to_json(&self) -> String;
}
```

`validate_shape` checks hex-length and non-empty fields.
No signature verification is performed in this slice — the
field is a placeholder so a certifying tool can fill it
later.

Honest scope: shape validation + JSON round-trip. No
reproducible-build check, no supply-chain audit, no
signature algorithm. ADR 0326 CI infra will integrate
the certification with build artefacts.

## Consequences

- 2 unit tests: valid manifest passes shape check and
  JSON round-trips; malformed commit SHA is rejected.
- Closes the P2 skeleton pass. All 12 P2 slices now carry
  a typed surface + honest-scope ADR + invariant tests.
