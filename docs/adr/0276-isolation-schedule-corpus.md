# ADR 0276 — In-tree ANSI isolation schedule corpus

## Status

Accepted. Closes the in-tree reference portion of
G1-REFERENCE.

## Context

Berenson et al.'s 1995 critique names concrete anomaly
schedules (P0–P4, A5A, A5B) that separate the SQL isolation
levels. The G1-REFERENCE qualification tail called for a
documented in-tree corpus of these schedules. Full
ANSI-reference-suite integration remains infrastructure
(ADR 0277).

## Decision

- New reference document
  [`BRADIAN_ISOLATION_SCHEDULES.md`](../reference/BRADIAN_ISOLATION_SCHEDULES.md)
  enumerates each schedule in Berenson shorthand and
  records what each Bradian engine actually does.
- New
  [`tests/isolation_schedules.rs`](../../crates/bradian-relational/tests/isolation_schedules.rs)
  exercises each schedule sequentially — the
  `DomainSqlEngine`'s per-engine operation mutex
  serializes sessions, so forcing a concurrent interleave
  in-process deadlocks. The honest finding is captured in
  the doc's Section 3: the mutex **prevents** each anomaly
  by making concurrency structurally impossible
  in-process.
- Each test asserts the structural invariant (no silent
  overwrite, no uncommitted visibility, stable
  within-session snapshot, consistent two-key snapshot,
  etc.) using the sequential shape that matches the
  Berenson schedule.

## Consequences

- G1-REFERENCE's in-tree portion is closed: 8 schedule
  tests + 1 reference document.
- Multi-process / distributed schedules remain open and
  documented as such; Bradian does not claim distributed
  isolation.
- External SQL-conformance suite (SQLLogicTest / pgTAP /
  DBMS-independent isolation suites) remains deferred to
  ADR 0277 — infrastructure integration, not code.
- A5B (write skew) has no code assertion beyond the setup:
  the ADR 0037 read-validation mode already proves the
  prevention in `HistorySqlEngine`; this corpus
  cross-references that test rather than duplicating it.
- 8 tests in `tests/isolation_schedules.rs` cover P0 /
  P1 / P2 / P3 / P4 / A5A / A5B + a seed round-trip
  baseline.
