# ADR 0315 — DHIS2 integration deployment path: skeleton

## Status

Accepted as a skeleton. Eighth slice of P2.

## Context

A customer-installable Bradian-for-DHIS2 bundle needs a
declarative manifest (DHIS2 base URL, catalog refresh
cadence, ingest cursor file, segment retention) that the
admin CLI can validate and apply. A full packaging +
upgrade + rollback path is Gate-sized.

## Decision

New module `bradian_dhis2::deployment` with a typed
manifest plus a shape-validating parser:

```rust
pub struct DeploymentManifest {
    pub dhis2_base_url:      String,
    pub catalog_refresh_sec: u32,
    pub ingest_cursor_path:  String,
    pub segment_retention:   RetentionPolicy,
}
pub enum RetentionPolicy {
    Forever,
    Days(u32),
    MaxBytes(u64),
}
pub fn load_manifest_json(bytes: &[u8]) -> DeploymentResult<DeploymentManifest>;
```

Honest scope:

- **Parse + shape validation only.** No HTTP fetch of
  DHIS2, no apply step, no upgrade / rollback, no systemd
  / docker artefacts.

## Consequences

- An admin CLI slice can take a `DeploymentManifest`
  without re-inventing JSON parsing.
- 2 unit tests: valid manifest round-trips; invalid URL
  surfaces a typed error.
