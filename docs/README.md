<p align="left">
  <img src="media/Bradian_Database_Systems_Primary_Horizontal_Blue.svg" width="330" alt="Bradian Database Systems" />
</p>

# Bradian Database Systems — Technical Documentation

**Owner:** Vitalworks Limited  
**Canonical source:** [socaya/bradian-db](https://github.com/socaya/bradian-db)  
**Primary architecture baseline:** Bradian Master Architecture & Software Requirements Specification **v1.1**

This directory contains the engineering specifications, architectural decisions, security design, and implementation evidence for Bradian Database Systems, the planned proprietary OLTP/OLAP/HTAP database. The architecture is **a target design**, not a statement that all its capabilities have been implemented.

## Read first

1. **[Master Architecture & SRS v1.1](Bradian_Master_Architecture_SRS_v1.1.md)** — normative component architecture, relational SQL, ACID/MVCC design, columnar analytics, HTAP consistency, cluster fabric, horizontal scaling, federation, security, disaster recovery, Bradian Studio, implementation guidance and acceptance gates.
2. **[Implementation audit and phased roadmap](implementation/BRADIAN_V1_1_AUDIT.md)** — maps existing source code to the SRS and records the not-yet-implemented engineering work.
3. **[Bradian security bootstrap guide](security/BOOTSTRAP_API_AUTH.md)** — local development authentication boundary and its important security limitations.
4. **[Relational catalog ADR](adr/0006-generic-relational-catalog.md)** — logical database/schema/table IDs, scope, constraints and migration limitations.
5. **[Portable page format ADR](adr/0007-portable-physical-page.md)** — versioned 4 KiB page format and corruption checks.
6. **[Native row WAL and checkpoint ADR](adr/0008-native-row-wal-checkpoint.md)** — synchronized single-key row mutations, page snapshots, fail-closed recovery and current limitations.

## Current milestone records

| Gate | Engineering scope | Current maturity |
|---|---|---|
| [BRD-G1](gates/BRD-G1-relational-catalog.md) | Relational catalog, typed column definitions, PK/FK metadata | In-memory schema management, no row data |
| [BRD-G2](gates/BRD-G2-page-format.md) | Versioned physical page representation and corruption detection | Page codec only; not yet durable storage |
| Future gates | WAL, transactional row storage, SQL engine, MVCC, HTAP, clusters, enterprise security | Must be implemented and qualified |

## Formal specification and assets

- [Master v1.1 — Markdown](Bradian_Master_Architecture_SRS_v1.1.md)
- [Master v1.1 — PDF](Bradian_Master_Architecture_SRS_v1.1.pdf)
- [Master v1.1 — editable DOCX](Bradian_Master_Architecture_SRS_v1.1.docx)
- [Master v1.0 — historical Bradian revision](Bradian_Database_Systems_Master_Architecture_SRS.md)
- [Approved blue horizontal logo](media/Bradian_Database_Systems_Primary_Horizontal_Blue.svg)
- [Approved white horizontal logo](media/Bradian_Database_Systems_Primary_Horizontal_White.svg)

**Diagram note:** The v1.1 Markdown references diagram image files that are not presently committed under `docs/diagrams/`. The PDF and DOCX are the retained figure-bearing master publications. Diagram-source assets should be restored in a later documentation maintenance change; do not imply the Markdown figure links are currently operational.

## Compatibility and historical material

Bradian originated from the DAP/DER Analytics Platform analytical prototype. The legacy documents below remain available **only as provenance and compatibility references**, not as the primary Bradian architecture:

- [Original DER Analytics Platform architecture](DER_Analytics_Platform_Architecture_and_Feature_Specification.md)
- [Original analytical requirements traceability](traceability.md)
- [Original analytical milestone records](gates/G1.md), [G2](gates/G2.md), and [G6](gates/G6.md)
- [Original DHIS2 analytics integration decision](adr/0005-dhis2-analytics-engine.md)

Legacy `dap-*` crate names, APIs, `.derc` storage and some DHIS2 integration identifiers are intentionally preserved to avoid breaking existing analytical data and clients. This does not change the product identity: **Bradian Database Systems**.

## Documentation policy

New engineering work should cite v1.1 requirement IDs and include design decisions and test evidence. Correctness, recovery, query isolation, privilege enforcement and cluster safety must be verified before a subsystem is described as completed. The source tree may contain interim implementations, and their limitations must remain explicit. Bradian's approved logo artwork must be reused without shape, spacing or lettering changes.
