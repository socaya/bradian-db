# ADR 0300 — DHIS2 metadata import / export round-trip

## Status

Accepted. Second slice of P1-3 (catalog + semantic layer),
closing P1-3.

## Context

`bradian_catalog::dhis2_adapter` already ingests a
`/api/metadata`-shaped JSON document into the in-memory
`Catalog`. P1-3's acceptance for this slice requires a
**round-trip**: import a reference bundle, export it back,
and show that re-importing the exported form reconstructs
an equivalent catalog.

Round-trip is lossy in principle — the adapter only
captures the fields the analytical engine reads, and the
catalog does not retain DHIS2-only metadata that was
dropped on import. The claim this ADR will verify is
weaker and honest: the exported bundle, when re-imported,
produces a catalog **equal on the observable subset**.

## Decision

1. Add typed enumerators to `Catalog` so the exporter can
   enumerate without groping into private state:
   `data_elements()`, `datasets()`, `category_combos()`,
   `category_option_combos()`, `org_units()`,
   `programs()`, `program_stages()`,
   `program_indicators()`, `indicators()`. Each returns an
   owned `Vec<T>`.

2. Add `dhis2_adapter::export_dhis2_bundle_json(catalog)`
   that serialises the catalog to a DHIS2-shaped JSON
   document matching the keys the importer reads:
   `dataElements`, `dataSets`, `categoryCombos`,
   `categoryOptionCombos`, `organisationUnits`,
   `programs`, `programStages`, `programIndicators`,
   `indicators`. Enumeration is sorted by UID for
   deterministic output.

3. Round-trip test
   `crates/bradian-catalog/tests/dhis2_roundtrip.rs`
   seeds the catalog with a reference 4-object bundle
   covering `dataElement`, `dataSet`, `categoryCombo` and
   `categoryOptionCombo`, exports it, re-ingests into a
   fresh catalog, enumerates both catalogs sorted by UID
   and asserts pairwise equality on every object type.

Scope limits:

- Observable subset only. DHIS2-only fields (translations,
  user groups, sharing ACLs, legacy Period strings not
  modelled by `PeriodType`) are deliberately NOT exported.
- No v41 / v40 version negotiation; the exporter emits one
  canonical shape.
- No incremental import (whole-bundle replacement
  semantics mirror the importer).

## Consequences

- The reference DHIS2 flow (import → process → export) is
  mechanically testable in-tree.
- 1 integration test covers DataElement / Dataset /
  CategoryCombo / CategoryOptionCombo round-trip.
- Subsequent slices (DHIS2-live HTTP push via
  `dhis2_http`) can trust the exporter's shape.
- Catalog gains 9 small typed enumerator methods.
