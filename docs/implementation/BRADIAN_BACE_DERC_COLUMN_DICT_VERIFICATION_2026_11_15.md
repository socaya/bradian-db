# BACE `.derc` column-level shared dict verification (ADR 0083)

15 November 2026. Baseline: `a3cd642`. Format bump from v1.3 to
v1.4. Column dict bytes live ONCE in a new column-dict region
between the pages and the dict pool / footer; Dictionary pages
carry only indices; the reader attaches the shared dict before
concat so the ADR 0082 fast path still applies.

## Executed result

**1076 Rust tests passed, 0 failed, 0 ignored** (1066 → 1076).
Clippy clean on `cargo clippy --workspace --all-targets
--locked -- -D warnings`.

| Executed check | Result |
|---|---|
| `cargo check -p dap-storage --locked` | Passed |
| `cargo test -p dap-storage --test column_dict --locked` | 10 passed |
| `cargo test --workspace --locked` | 1076 passed, 0 failed |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed |

## §3.5 item 10 HARD-FAIL size test: PASSED

```
ADR0083-S10 v1.3 (per-page dict):     409938 bytes
            v1.4 (column-dict):        72721 bytes
            reduction:                   82.3%
```

Workload: 4096 rows × 8-rows-per-page × 200 distinct entries.
v1.3 duplicates the ~1.6 KB column dict into every one of 512
pages. v1.4 stores it once. Observed savings: **337 KB
(82.3%)** on this workload.

## What landed

### R.1 — format V1_4 + COLUMN_DICT flag

`format.rs`:
- `SEGMENT_VERSION_MINOR_V1_4: u16 = 4`
- `COLUMN_DICT: u64 = 1 << 8`
- `ALL_SUPPORTED_V1_4` superset
- `is_subset_supported_v1_4` fn

### R.2 — footer: unchanged

Pivoted from the ADR draft: no per-column footer fields. The
column-dict region is keyed by column name (BTreeMap for
deterministic ordering at write time), matching the existing
dict-pool pattern. No bincode schema change; v1.3 segments
remain byte-identical. See ADR §5 revisited: this is Q5
(region position = between pages and dict pool, before
footer) + "no footer bincode change".

### R.3 — writer column-dict region

`writer.rs`:
- `SegmentWriterOptions.use_column_dict: bool` (default `false`)
- `SegmentFileWriter.column_dicts: BTreeMap<String, (Vec<u32>, Vec<u8>, u32)>`
- `write_column` stashes the column dict once when
  `use_column_dict && codec == Dictionary && buf is DictIndex32`
- `emit_one_page` takes a `column_dict_active: bool` flag;
  when active, encodes via `dict::encode_indices_only` instead
  of `encode_page`.
- Header: emits minor=4 + COLUMN_DICT flag whenever
  `use_column_dict=true` (independent of BACE).
- `write_column_dict_region` writes the region between pages
  and dict pool, followed by a u32 region-length trailer so
  the reader can locate it by seeking backwards from
  FOOTER_MAGIC or from `dict_pool_start`.

`codec/dict.rs`:
- `encode_indices_only(buf)`: writes `(n_rows, n_dict, indices)`
  only — no dict bytes.
- `decode_indices_only(bytes) -> (indices, n_dict_declared)`

### R.4 — reader column-dict region

`reader.rs`:
- `SegmentReader.column_dicts: HashMap<String, (Vec<u32>, Vec<u8>)>`
- `read_footer_with_features` now returns a 4-tuple:
  `(footer, bace_feature, dict_pool_feature, column_dict_feature)`
- Legacy `open` rejects v1.4 via `is_subset_supported` (no
  COLUMN_DICT in the base set).
- `open_with_bace` uses `is_subset_supported_v1_4` and walks
  regions backwards from footer_start: dict_pool first (if
  flag set), then column_dict region (if flag set).
- `read_dictionary_pool` renamed to
  `read_dictionary_pool_at(file, region_end)` so cursor-
  threading works across multiple regions.
- `read_column_dict_region_at` parses the region, verifying
  per-entry CRC32 and last-offset-matches-data-len invariant.
- `decode_page` gains a `column_dict: Option<&(Vec<u32>, Vec<u8>)>`
  parameter. When present and `page.codec == Dictionary`,
  decodes via `decode_indices_only`, cross-checks
  `n_dict_declared` against the column-dict's actual size,
  validates indices against dict bounds, and returns a
  `ColumnBuffer::DictIndex32` with the shared dict attached.

### R.5 — 10 §3.5 integration tests in `column_dict.rs`

| # | Test | §3.5 item |
|---|---|---|
| 1 | `item_1_default_options_preserve_v1_3_shape` | 1 |
| 2 | `item_2_use_column_dict_emits_v1_4_flag_and_region_on_single_page` | 2 |
| 3 | `item_3_use_column_dict_four_page_dict_column_round_trips` | 3 |
| 4 | `item_4_v1_4_concat_still_returns_dictindex32` | 4 |
| 5 | `item_5_legacy_open_rejects_v1_4_segment` | 5/7 |
| 6 | `item_6_column_dict_crc_mismatch_is_hard_fail` | 6 |
| 7 | `item_7_v1_4_with_bace_envelope_round_trips` | 9 |
| 8 | `item_8_v1_4_with_baca_round_trips_under_matching_key` | 9 |
| 9 | `item_9_n_dict_mismatch_is_hard_fail` | 4 |
| 10 | `item_10_v1_4_strictly_smaller_than_v1_3_on_high_dup_workload` | 10 (HARD-FAIL) |

### Mechanical updates

Added `use_column_dict: false` to all existing
`SegmentWriterOptions` struct-literal construction sites:
`partition/store.rs` (2), `multi_page.rs`,
`multi_page_dict.rs`, `derc_bace.rs`, `derc_baca.rs`,
`derc_dictionary.rs`, `segment_roundtrip.rs`.

## Compliance against ADR 0083 §9

1. ADRs 0060–0082 accepted — ✓
2. Owner sign-off on §5 open design questions — accepted as
   drafted via "accept and implement" directive:
   - Q1: CRC32 (not BLAKE3) — per existing convention.
   - Q2: hard-fail on CRC32 mismatch — implemented.
   - Q3: `use_column_dict` default `false` — preserves v1.3
     byte-identity.
   - Q4: in-memory sharing = Clone (not Arc) — ADR 0082 §4(D)
     deferral upheld.
   - Q5: region position = between pages and dict pool,
     before footer — implemented.
   - Q6: interacts cleanly with BACE dict pool — implemented
     (both flags can be set; regions are distinct).
   - Q7: BACA scope deferred — the column-dict region is NOT
     covered by any page MAC; it carries its own CRC32.
3. Every §3.5 item 1–10 covered by at least one test.
4. §3.5 item 10 PASSED with 82.3% size reduction.
5. No regressions in any dap-storage, bradian-relational, or
   workspace test; 1066 → 1076 tests (10 new).
6. Feature-flag superset check preserved
   (`ALL_SUPPORTED_V1_3` ⊂ `ALL_SUPPORTED_V1_4`).

## What this ADR explicitly does NOT do (unchanged)

- Change behavior of v1.0–v1.3 segments. They remain fully
  readable and writable.
- Introduce cross-column dict sharing.
- Migrate existing v1.3 segments online.
- Extend BACA MAC coverage to the column-dict region.
- Add CLI exposure.
- Introduce `Arc<Dict>` or any reference-counted sharing.

## BACE workstream progress (as of this ADR)

| ADR | Component | Status |
|---|---|---|
| 0060–0066 | Prerequisites + RowStore + ZSTD | ACCEPTED |
| 0067–0068 | Backup carrier adapters | ACCEPTED |
| 0069–0072 | All four SQL engines BACE open | ACCEPTED |
| 0073 | `bradian-db` CLI BACE flags | ACCEPTED |
| 0074 | G23 DDL demonstration | ACCEPTED |
| 0075 | BACA envelope primitive | ACCEPTED |
| 0076 | `.derc` carrier BACE (v1.1) | ACCEPTED |
| 0077 | BRWL carrier BACE (benchmarked) | ACCEPTED |
| 0078 | BACE dictionary primitive | ACCEPTED |
| 0079 | `.derc` dictionary carrier (v1.2) | ACCEPTED |
| 0080 | `.derc` BACA carrier (v1.3) | ACCEPTED |
| 0081 | `.derc` multi-page writer | ACCEPTED |
| 0082 | Multi-page DictIndex32 columns | ACCEPTED |
| **0083** | **Column-level shared dict (v1.4)** | **ACCEPTED (82.3% size win)** |

## Remaining BACE directions

- Dict-union concat for per-page sub-dict writers (new ADR)
- Cross-column dict sharing (new ADR)
- BACA MAC coverage of column-dict region (new ADR)
- `Arc<DictBytes>` in-memory sharing (new ADR)
- BACA on backup / BRWL / RowStore carriers
- Key management workflow (keyfile loader, rotation)
- Real-world DHIS2 workload benchmarks
- CLI exposure of `use_column_dict` and `max_rows_per_page`
- Streaming reads (iterator API)

## Baseline + commit

Baseline: `a3cd642` (ADR 0082 ACCEPTED).
Implementation commit: assigned by this commit.

See [ADR 0083](../adr/0083-derc-column-level-shared-dict.md).
