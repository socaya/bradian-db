# ADR 0115 — Data-root format version manifest (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch G G1 — upgrade /
version-compat primitive.

## Scope

- `format.manifest` JSON file at every Bradian data root.
- `FormatManifest { product, version, created_at_ms,
  last_opened_at_ms }`.
- `ensure_manifest(root)` creates-or-opens; rejects
  too-new (operator must upgrade) and too-old (downgrade
  explicitly refused).
- `wrong product` rejected to prevent cross-product
  confusion (other DBs could put a `format.manifest` with a
  different product string).
- `CURRENT_FORMAT_VERSION: u32 = 1`.

## Out of scope

- **Engine opt-in wiring** — RowStore / Catalog / History
  engines continue to open without manifest checks.
  Attaching this check to their `open()` entry points is a
  mechanical follow-up.
- **Format migration logic** — version bumps today are
  reject-on-open. A future ADR could add in-place migration
  for selected bumps (e.g., `.brcs` v1 → v2).
- **Signed manifest** — tamper-evidence via BLAKE3 keyed MAC
  is deferred to a BACE-style extension.

## Compliance

1. Fresh root: manifest created at `CURRENT_FORMAT_VERSION`.
2. Second open: `last_opened_at_ms` advances.
3. On-disk newer than build max: `TooNew`.
4. On-disk older than build min: `TooOld`.
5. Wrong product string: `WrongProduct`.
6. 5 unit tests in `src/format_manifest.rs`.
