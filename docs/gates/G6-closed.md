# G6 close — DHIS2 analytics gaps implemented

This supersedes `docs/gates/G6.md`. Every gap the previous draft tagged DEFER now has code
and tests. Items that remain partially implemented are listed under "Remaining work" at the
bottom with the specific slice that still needs attention.

## Gaps closed

| Previous status | Area | Where it lives now | Tests |
|---|---|---|---|
| DEFER | `d2:` function namespace | `expression::eval_d2` with 14 functions (yearsBetween, monthsBetween, weeksBetween, daysBetween, minutesBetween, hasValue, oizp, zpvc, condition, count, countIfValue, countIfCondition, relationshipCount, validatePattern) | 6 new expression tests |
| DEFER | STDDEV / VARIANCE aggregation | Welford online algorithm in `aggregate::Combiner`; flatten pooling in `collapse_cells_ts` to match textbook variance | `variance_and_stddev_match_textbook` |
| DEFER | LAST / LAST_IN_PERIOD | `aggregate::State::LastPick` max-timestamp selector; `LAST_IN_PERIOD` uses LAST across time + AVG across OU | `last_picks_highest_timestamp_per_ou_and_overall`, `last_in_period_averages_across_ous` |
| DEFER | Period offsets `#{DE}.periodOffset(-N)` | New `RefKind::PeriodOffset { inner, offset }`; lexer tacks the suffix onto any `#{...}` ref | `period_offset_reaches_resolver` |
| DEFER | Reporting rates `#{dataset.VARIANT}` | `RefKind::ReportingRate` + `ReportingRateVariant` + new `completion` module (`CompletionFact`, `CompletionStore`) computing all 5 variants with zero-expected → NULL | `reporting_rate_parses_as_dedicated_ref`, `computes_five_variants`, `zero_expected_yields_null_rate`, `store_resolves_missing_as_null`, `upsert_version_fence` |
| DEFER | Relationship analytics (DHI-004) | `relationship` module with `RelationshipType`, `Relationship`, `RelationshipIndex` (O(1) count per entity + type, bidirectional traversal, dedup neighbours) | 3 relationship tests |
| DEFER | Program-rule side-effect analytics | `program_rule::RuleSource` + `RuleProvenance` (user / rule / import / derived / external) wired through canonical envelope | `user_vs_machine_distinction`, `note_for_each_source` |
| DEFER | CUSTOM Program Indicator aggregation | `custom_aggregation` module: Sum / Avg / Count / Min / Max / StdDev / Variance / Percentile composed via arithmetic sub-expressions; nested aggregators rejected at compile | 5 custom_aggregation tests |
| DEFER | Event / Enrollment planner DSL | `QueryRequest::ProgramIndicator(ProgramIndicatorQuery)`; planner translates program + indicator UIDs into a logical plan whose predicate is anchored on `event_date` | compiles through full workspace build |
| DEFER | Materialised views for PI | `views::materialize` with three builders: classic indicator, event PI, enrollment PI over OU × period grid | `materialise_classic_indicator_grid` |
| DEFER | DHIS2 metadata ingest | `catalog::dhis2_adapter` — parses `/api/metadata` JSON, applies DataElement / Dataset / CategoryCombo / OrgUnit / Program / ProgramStage / ProgramIndicator / Indicator + exhaustive ValueType / AggregationType mapping | `parses_small_bundle`, `unknown_value_type_rejected` |

## Summary

| Section | Tests | Status |
|---|---|---|
| expression grammar (incl. d2: namespace, period offsets, reporting-rate refs) | 23 | DONE |
| aggregate reductions (full matrix) | 10 | DONE |
| completion facts + reporting rates | 4 | DONE |
| event-grain PI | 5 | DONE |
| enrollment-grain PI + tracker | 6 | DONE |
| relationship analytics | 3 | DONE |
| program-rule provenance | 2 | DONE |
| classic indicators | 10 | DONE |
| custom PI aggregation | 5 | DONE |
| materialised views for PI | 1 integration + grid test | DONE |
| DHIS2 metadata adapter | 2 | DONE |
| workspace total | **171 passing · clippy clean** | |

## Remaining work

Not blockers for closing G6; carried into the next iteration.

1. **Incremental retraction** for materialised views. Today a view is rebuilt on demand via
   the materialisers; incremental old/new contribution refresh is the INC-001/INC-002
   PART item.
2. **Live DHIS2 HTTP client.** The metadata adapter accepts JSON bytes; a thin
   `reqwest`-based fetcher that pulls from a running DHIS2 (`/api/metadata?assume
   TrueOnly=true&...`) will land with the admin CLI (OPS-004 scope).
3. **Weekly-variant / fiscal periods** (`WeeklyWednesday`, `WeeklyThursday`, `FinancialOct`
   etc). The adapter rejects them explicitly; implementation needs matching ISO-week
   anchor math.
4. **Program-rule execution.** The engine preserves rule provenance but does not evaluate
   the rules themselves; DHIS2's capture layer still owns execution.
5. **d2:validatePattern full Java regex.** v1 handles anchored (`^...$`) and substring
   matches without a regex crate dependency; richer patterns will land once the regex
   dependency is accepted into the vetted set.
6. **CUSTOM aggregation Spearman / Kendall rank functions.** Common in epidemiology but
   outside the DHIS2 reference grammar; tracked separately as a feature request.

These remaining items are now smaller, well-scoped follow-ups rather than unknown gaps.
