# ADR 0315 — DHIS2 integration deployment path

## Status

Accepted. Replaces the 2026-11-24 skeleton with a **real
materialising `apply_to` + `load_applied` + `verify` flow**
that produces an operator-ready deployment directory.

## Context

The skeleton parsed and shape-validated a DHIS2
deployment manifest but did nothing with it. A real
deployment command needs to put files on disk, ensure
writable paths, and give a reviewer a mechanical way to
detect drift between the declared and the applied
configuration.

## Decision

Module `bradian_dhis2::deployment`:

```rust
pub struct DeploymentManifest {
    pub dhis2_base_url:      String,
    pub catalog_refresh_sec: u32,
    pub ingest_cursor_path:  String,    // relative to deployment root
    pub segment_retention:   RetentionPolicy,
}

pub enum RetentionPolicy { Forever, Days { days }, MaxBytes { bytes } }

pub fn load_manifest_json(bytes: &[u8]) -> BradianResult<DeploymentManifest>;
impl DeploymentManifest {
    pub fn apply_to(&self, root: impl AsRef<Path>) -> BradianResult<DeploymentLayout>;
    pub fn to_json(&self) -> String;
}
pub fn load_applied(root: &Path) -> BradianResult<(DeploymentManifest, DeploymentLayout)>;
pub fn verify      (root: &Path, expected: &DeploymentManifest) -> BradianResult<()>;
```

Validation enforces:

- `dhis2_base_url` must start with `http://` or `https://`.
- `catalog_refresh_sec > 0`.
- `ingest_cursor_path` is non-empty, **relative** (not
  absolute), and contains no `..` components (both
  escape-prevention and operator-mistake detection).
- `RetentionPolicy::Days { days: 0 }` and
  `MaxBytes { bytes: 0 }` are rejected.

`apply_to(root)`:

- Creates `root/` + parent directories of the cursor path.
- Writes `{root}/manifest.json` via atomic tmp-rename.
- **Idempotent on the cursor file**: creates it only if
  absent (zero bytes). A second apply over an existing
  cursor with operator-written bytes **preserves** them.
- Writes `{root}/retention.hint` (human-readable summary)
  via atomic tmp-rename.
- Returns a `DeploymentLayout` enumerating every path
  that was touched.

`load_applied(root)`:

- Re-reads + re-validates the manifest.
- Fails closed if the ingest cursor file is missing.

`verify(root, expected)`:

- Loads the applied manifest and asserts byte-equivalence
  with `expected`.

## Scope

**In scope:**
- Durable tmp-rename materialisation of manifest.json +
  retention.hint.
- Idempotent cursor-file creation that never clobbers
  operator-written bytes.
- Validation of URL scheme, relative-path safety,
  retention sanity.
- `verify` detecting drift (manifest rewritten with
  different bytes).
- 9 unit tests: valid-manifest round-trip, invalid URL
  scheme rejection, absolute cursor rejection, `..`
  component rejection, zero-retention rejection, apply_to
  materialises manifest + cursor + hint, apply-is-
  idempotent-preserves-cursor-bytes, verify-detects-drift,
  load_applied rejects missing cursor.

**Out of scope** (dedicated follow-ups):
- Live HTTP fetch of DHIS2 metadata (ADR 0300 adapter
  does the parse; wiring it into apply is a next slice).
- Systemd unit / docker-compose generation.
- Rollback to a prior applied manifest snapshot.

## Consequences

- An admin CLI can shell out to `apply_to`, then later
  `verify` against a committed manifest — the standard
  "record of intent + proof of applied state" flow.
- The 2026-11-24 "SKELETON" status on 0315 is retired.
