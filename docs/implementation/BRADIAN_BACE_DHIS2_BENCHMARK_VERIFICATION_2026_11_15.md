# BACE DHIS2-like benchmark verification (ADR 0094)

15 November 2026. Baseline: `9b8c31e`. Synthetic workload only;
real DHIS2 data not in this repo.

## Workload

- Deterministic facts via `generate_facts(seed=42, 50 OUs, 12
  periods, 20 DEs, 36_000 rows)`.
- Schema: `ou UID, period PERIOD, de UID, coc UID, value F64`
  (5 columns).
- Rows: 36,000.
- 180,000 column-row cells per read.

## Observed sizes + timings

```
[ADR0094 WRITE] v1.0 legacy              363 820 bytes   31.8 ms
[ADR0094 WRITE] v1.1 BACE                364 489 bytes   25.0 ms
[ADR0094 WRITE] v1.2 BACE+dictpool       365 690 bytes   16.1 ms
[ADR0094 WRITE] v1.4 column-dict         886 116 bytes   31.1 ms
[ADR0094 WRITE] v1.4 + BACA              931 156 bytes   33.0 ms

[ADR0094 READ ] v1.0 legacy                1.0 ms
[ADR0094 READ ] v1.1 BACE                  1.0 ms
[ADR0094 READ ] v1.2 BACE+dictpool         1.0 ms
[ADR0094 READ ] v1.4 column-dict           7.6 ms
[ADR0094 READ ] v1.4 + BACA                8.3 ms
```

## Honest reading

**The v1.4 column-dict format is not a universal win.** On this
workload it is **2.4× LARGER** than v1.0. Why: with
`max_rows_per_page = 64` across 36k rows, each dict-eligible
column emits 563 pages. Per-page index overhead (64 rows × 4
bytes = 256 bytes) × 4 dict-eligible columns × 563 pages ≈ 576
KB of index bytes alone, dominating the modest dict-pool savings
on this schema.

The ADR 0083 §3.5 item 10 hard-fail test — where v1.4 was 82.3%
SMALLER than v1.3 — used a very different workload: **4096 rows
× 8-rows-per-page × 200 distinct entries** on a single dict
column with an 8 KB dict. That workload's duplication cost
(~4 MB across 512 pages) dwarfed the index overhead, so v1.4
dominated.

The honest operator guidance:

- **Column-dict wins** when: column dict is LARGE (hundreds of
  distinct entries × long strings), split is small enough that
  v1.3 per-page duplication becomes significant, and per-page
  index overhead stays below the duplication savings.
- **Column-dict loses** when: schema has many dict-eligible
  columns but small dicts, OR when `max_rows_per_page` is
  chosen aggressively small on modest workloads.

The read overhead on v1.4 (7.6 ms vs 1.0 ms) comes from the
563-page reconstruction per column; the single-clone fast path
(ADR 0087) still reads page-by-page.

## Compliance against ADR 0094 §2

1. Bench compiles + runs — ✓.
2. All five variants round-trip without error — ✓.
3. Observed sizes recorded above — ✓.
4. 1137 pre-ADR-0094 workspace tests pass.
5. Clippy clean (after unused-import cleanup).

## Future benchmark directions

- Real DHIS2 production data (requires out-of-repo artifact).
- Vary `max_rows_per_page` across {8, 64, 512, 4096} to show
  the v1.4 sweet-spot curve.
- Compare against Parquet/zstd on the same workload.
- Per-column dict-cardinality sweeps.
- Hot-path read throughput at larger row counts (100M+ rows).
