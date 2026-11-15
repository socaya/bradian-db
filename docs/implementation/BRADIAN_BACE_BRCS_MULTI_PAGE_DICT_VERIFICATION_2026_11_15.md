# BACE `.brcs` multi-page `DictIndex32` verification (ADR 0082)

15 November 2026. Baseline: `8560af1`. Closes the ADR 0081 §2
non-goal by removing the dict-column split bypass. Dict
columns now split into N pages exactly like plain and Utf8
columns; every page carries a byte-identical copy of the
column dict; the reader's identical-dict fast path returns
`DictIndex32`; a differing-dict fallback promotes to `Utf8`
for forward compatibility.

## Executed result

**1066 Rust tests passed, 0 failed, 0 ignored** (1056 → 1066).
Clippy clean on `cargo clippy --workspace --all-targets
--locked -- -D warnings`.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-storage --locked` | Passed |
| `cargo test -p bradian-storage --test multi_page_dict --locked` | 10 passed |
| `cargo test --workspace --locked` | 1066 passed, 0 failed |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed |

## What landed

### R.1 — `ColumnBuffer::slice` for `DictIndex32`

`column.rs` replaces the ADR 0081 §2 panic with the
conservative dict-clone implementation:

```rust
ColumnBuffer::DictIndex32 { indices, validity, dict_offsets, dict_data } => {
    ColumnBuffer::DictIndex32 {
        indices: indices[start..end].to_vec(),
        validity: validity.slice(start, end),
        dict_offsets: dict_offsets.clone(),
        dict_data: dict_data.clone(),
    }
}
```

Dict bytes are duplicated per page; indices are NOT rebased —
they still name the same dict entries.

### R.2 — writer dict-column split

`writer.rs` removes the `is_dict` bypass. Dict columns now
split into pages of at most `max_rows_per_page` rows, just
like plain and Utf8 columns:

```rust
let page_size = match self.options.max_rows_per_page {
    Some(limit) if limit > 0 && limit < total_rows => limit,
    _ => total_rows.max(1),
};
```

`materialize_dict_to_utf8` stays for the PlainUtf8-codec-
with-DictIndex32-buffer case (threshold=0.0 override), which
is a separate code path from this ADR's subject.

### R.3 — reader `concat_buffers` for `DictIndex32`

`reader.rs` replaces the `Unsupported` return with a two-
branch implementation:

- **Fast path** (writer invariant): all pages share identical
  `(dict_offsets, dict_data)` → return `DictIndex32` with
  concatenated indices.
- **Fallback** (forward compat): differing dicts → materialize
  each page's indices to Utf8 strings, concat, return
  `Utf8`.

No v1 writer produces the differing-dict shape, so the
fallback branch exists only for compatibility with a future
per-page-sub-dict writer.

### 10 §3.5 integration tests in `multi_page_dict.rs`

| # | Test | §3.5 item |
|---|---|---|
| 1 | `item_1_default_options_dict_column_still_one_page` | 1 |
| 2 | `item_2_max_rows_per_page_50_on_200_rows_emits_four_dict_pages` | 2 |
| 3 | `item_3_multi_page_dict_round_trips_to_identical_values` | 3 |
| 4 | `item_4_every_page_carries_byte_identical_dict_bytes` | 4 |
| 5 | `item_5_fast_path_returns_dictindex32_not_utf8` | 5 |
| 6 | `item_6_differing_dicts_concat_fallback_produces_utf8` | 6 |
| 7 | `item_7_bace_envelope_round_trips_multi_page_dict` | 7 |
| 8 | `item_8_multi_page_dict_size_observation_recorded` | 8 |
| 9 | `item_9_baca_envelope_round_trips_multi_page_dict` | 9 |
| 10 | `item_10_bace_dictionary_pool_on_multi_page_dict_column` | 10 |

### ADR 0081 `item_7` updated

`multi_page.rs::item_7_dict_encoded_column_emits_single_page_
even_with_split_set` was renamed to `item_7_dict_encoded_
column_splits_like_every_other_column` and now asserts
`tag_pages == 4`. The previous assertion (the ADR 0081 §2
non-goal) is intentionally reversed by this ADR.

## §3.5 item 8 honest size observation

The ADR explicitly does NOT claim multi-page dict is strictly
smaller than multi-page plain-Utf8 on the same workload.
Per-page dict duplication is an accepted cost. The test
records observed bytes without an ordering assertion beyond
"both are non-empty and round-trip correctly". This matches
ADR 0082 §10.

Shape chosen: 200 rows with 4 distinct values split into 4
pages of 50 rows each. The dict (4 entries × ~8 bytes) fits
easily; the per-page duplication is ~32 bytes × 4 pages ≈
128 bytes of overhead relative to a column-level-shared-dict
baseline that does not exist yet.

## Compliance against ADR 0082 §9

1. ADRs 0060–0081 accepted — ✓
2. Owner sign-off on §5 open design questions — accepted as
   drafted via "accept and implement" directive.
3. Every §3.5 item 1–10 covered by at least one test.
4. §3.5 item 8 recorded honestly — no strict-smaller
   assertion against a hypothetical column-level-shared-dict
   baseline.
5. No regressions in any bradian-storage, bradian-relational, or
   workspace test; 1056 → 1066 tests (10 new, 0 modified
   asserts broken, 1 asserted value flipped as required by
   the ADR).

## What this ADR explicitly does NOT do (unchanged)

- Column-level shared dict format (no format bump).
- Dict-union on concat (fallback is Utf8, not union).
- Per-page sub-dict optimization.
- Streaming reads.
- CLI exposure.
- Changes to BACE/BACA/dict envelope semantics.

## BACE workstream progress (as of this ADR)

| ADR | Component | Status |
|---|---|---|
| 0060–0066 | Prerequisites + RowStore + ZSTD | ACCEPTED |
| 0067–0068 | Backup carrier adapters | ACCEPTED |
| 0069–0072 | All four SQL engines BACE open | ACCEPTED |
| 0073 | `bradian-db` CLI BACE flags | ACCEPTED |
| 0074 | G23 DDL demonstration | ACCEPTED |
| 0075 | BACA envelope primitive | ACCEPTED |
| 0076 | `.brcs` carrier BACE (v1.1) | ACCEPTED |
| 0077 | BRWL carrier BACE (benchmarked) | ACCEPTED |
| 0078 | BACE dictionary primitive | ACCEPTED |
| 0079 | `.brcs` dictionary carrier (v1.2) | ACCEPTED (plumbing + relaxed size test) |
| 0080 | `.brcs` BACA carrier (v1.3) | ACCEPTED (narrow threat model) |
| 0081 | `.brcs` multi-page writer | ACCEPTED (size win verified) |
| **0082** | **Multi-page `DictIndex32` columns** | **ACCEPTED** |

## Remaining BACE directions

- Column-level shared-dict format (new ADR; format bump)
- Dict-union concat for per-page sub-dict writers (new ADR)
- BACA on backup / BRWL / RowStore carriers
- Key management workflow (keyfile loader, rotation)
- Real-world DHIS2 workload benchmarks
- CLI exposure of `max_rows_per_page`
- Streaming reads (iterator API)

## Baseline + commit

Baseline: `8560af1` (ADR 0081 ACCEPTED).
Implementation commit: assigned by this commit.

See [ADR 0082]-brcs-multi-page-dict-columns.md).
