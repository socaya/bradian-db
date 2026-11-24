# ADR 0295 — HTAP freshness via LSN lag annotation

## Status

Accepted. Fourth and final slice of P1-1.

## Context

ADR 0294 lets a materialised view consume commit receipts
and stay invertibly incremental with the shared WAL
domain. An HTAP reader deciding whether to serve a result
from the view OR fall back to a direct row scan needs an
**explicit** freshness signal: "the view is current to
LSN V; the source is at LSN S; the lag is S − V LSNs."

`bradian-views::freshness::FreshnessStamp` is a time-based
stamp intended for the DHIS2 aggregation pipeline — it
compares millisecond watermarks against a metadata
fingerprint. That is not the right primitive for a
transactional HTAP reader: millisecond timestamps drift
under NTP, and a view that hasn't absorbed the most recent
commit does not know "what time it is". LSN monotonicity
from the shared WAL is the authoritative clock.

## Decision

Add a new, deliberately small type in
`bradian-views::htap_freshness`:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HtapFreshness {
    pub view_lsn: u64,
    pub source_lsn: u64,
}
impl HtapFreshness {
    pub fn lag_lsn(&self) -> u64 { source - view }
    pub fn is_fresh(&self) -> bool { view == source }
    pub fn classify(&self) -> HtapStaleness { ... }
}
pub enum HtapStaleness { Fresh, Behind { lag: u64 }, Ahead }
```

`Ahead` captures the "view LSN > source LSN" case
(impossible in a single-writer domain but possible across
a failover or a stale cache) — the classifier calls it out
rather than silently clamping.

Scope limits:

- No automatic hook into `MultiTableStore` or
  `DomainSqlEngine`. Callers build the stamp from
  `receipt.lsn` and `store.last_lsn()` at read time. The
  wiring into a production HTAP router is a follow-up.
- No cross-domain freshness, no distributed clock, no
  physical-time annotation. This primitive is LSN-only.
- Integration with the time-based `FreshnessStamp`
  (DHIS2 pipeline) is not attempted; the two primitives
  coexist for different consumers.

## Consequences

- P1-1 is now DONE. An HTAP read path can produce an
  explicit "serve from view (fresh) / serve from view
  with lag of N LSNs / fall back to scan" decision using
  one `u64` comparison.
- `bradian-views` is re-exported with `HtapFreshness` /
  `HtapStaleness`.
- 1 integration test
  (`htap_freshness_lsn.rs`) builds a maintained view,
  drives two commits, and asserts the stamp advances /
  lags / re-freshens correctly, including the `Ahead`
  classification.
- Zero behaviour change for existing callers; the new
  type is purely additive.
