# ADR 0085 — `bradian-db` CLI exposure of column-dict + multi-page knobs (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-15 and implemented.** 1089 workspace tests passed (1086 → 1089). Clippy clean.

ADR 0081 (multi-page writer) and ADR 0083 (v1.4 column-dict
format) added operator-visible knobs to `SegmentWriterOptions`
— `max_rows_per_page: Option<usize>` and `use_column_dict:
bool`. Neither is reachable from the `bradian-db` CLI today;
`partition_durability` tests force the options at the code
level, and operators cannot select them without editing Rust.

This ADR surfaces both knobs on the `bradian-db compact-
partition` + `export-snapshot` commands.

Owner review required before any code lands per CLAUDE.md §4.9.

## 1. Context

- `SegmentWriterOptions.max_rows_per_page: Option<usize>`:
  controls the ADR 0081 split. Default `None` = single page
  per column. Operators who want BACE dict-pool amortization
  (ADR 0079) or column-dict shared compression (ADR 0083)
  typically want small pages (e.g., 8–128 rows/page) so
  per-page redundancy compresses well.
- `SegmentWriterOptions.use_column_dict: bool`: controls
  the ADR 0083 v1.4 format. Default `false` keeps v1.3
  per-page dict duplication. Operators get **~82% size
  reduction** on dict-heavy workloads by setting this to
  `true` (the ADR 0083 §3.5 item 10 hard-fail test
  observation).
- Both knobs are plumbed into `partition::store.rs` via
  `SegmentWriterOptions` literals at two sites (initial
  partition flush + compaction). The partition-level
  `PartitionOptions` struct does NOT thread them today;
  it hard-codes `None` + `false`.
- Admin CLI: `bradian-db compact-partition` + `bradian-db
  export-snapshot` are the two entry points where a new
  `.derc` write happens at operator request.

## 2. Scope of this ADR

**In scope:**

- Add two fields to `PartitionOptions`:
  - `max_rows_per_page: Option<usize>` (default `None`)
  - `use_column_dict: bool` (default `false`)
- `Partition` stores them and passes them into every
  `SegmentWriterOptions` construction site in `store.rs` (2
  sites).
- Add two flags to `bradian-db compact-partition`:
  - `--max-rows-per-page <N>` — maps to `Some(N)` or `None`.
  - `--use-column-dict` — bool flag, default off.
- Same two flags on `bradian-db export-snapshot`.
- Integration tests: compaction produces a v1.4 segment with
  `COLUMN_DICT` flag set when `--use-column-dict` is passed;
  page count matches `--max-rows-per-page` hint.
- Help text documents the ADR 0083 size-win range so
  operators understand the tradeoff.

**Explicitly out of scope** (future ADRs):

- Threading the knobs into **live partition writes** (flush
  path). The default-off semantics stay there: live ingest
  continues to use `None`/`false` unless the CLI is extended
  to configure the live partition too. This ADR limits
  behavior change to `compact-partition` + `export-snapshot`,
  both of which are explicit operator actions.
- A persistent per-partition setting (saved in the manifest
  or a config file). Operators pass the flags each time.
  Future ADR could add a `.bradian/config.toml` that defaults
  these knobs per partition.
- Automatic selection (e.g., "pick N based on row count").
  Operator-controlled only.
- Serving-layer CLI changes.
- Changes to the `serve` or Dhis2 commands.

## 3. Proposed design

### 3.1 `PartitionOptions` extension

```rust
#[derive(Clone, Debug)]
pub struct PartitionOptions {
    pub dataset_id: String,
    pub partition_id: String,
    pub schema: Schema,
    pub primary_key_columns: Vec<String>,
    pub flush_rows: usize,
    pub flush_bytes: usize,
    /// ADR 0085 — propagated to SegmentWriterOptions.
    pub max_rows_per_page: Option<usize>,
    /// ADR 0085 — propagated to SegmentWriterOptions.
    pub use_column_dict: bool,
}
```

Both default to `None` / `false` — pre-ADR-0085 behavior.

### 3.2 `store.rs` propagation

The two `SegmentWriterOptions` literals in `write_delta_to_
segment` and `compact_visible_segments` consume
`self.options.max_rows_per_page` and
`self.options.use_column_dict` instead of hard-coded `None`
+ `false`.

### 3.3 CLI flags

```rust
// CompactArgs + ExportSnapshotArgs:
#[arg(long, help = "Pages per column (default: one page per column); \
                    small values (8-128) improve BACE dict compression")]
max_rows_per_page: Option<usize>,

#[arg(long, help = "Use the v1.4 column-level shared dict format \
                    (ADR 0083); ~82% size reduction on dict-heavy \
                    workloads with small --max-rows-per-page")]
use_column_dict: bool,
```

`compact_partition` + `export_snapshot` in `lib.rs` thread
these into the `PartitionOptions` they construct.

### 3.4 Compliance obligations (implementation test checklist)

1. Default: `bradian-db compact-partition` without the new
   flags produces byte-identical output to pre-ADR-0085
   (modulo the always-varying created_at header timestamp
   field).
2. `--max-rows-per-page 8` produces a segment where every
   column has `ceil(rows / 8)` pages.
3. `--use-column-dict` produces a segment with minor
   version 4 and the `COLUMN_DICT` feature flag set, and the
   column-dict region present.
4. `--max-rows-per-page 8 --use-column-dict` produces a
   segment that's strictly smaller than the same workload
   without `--use-column-dict`, matching the ADR 0083 size
   claim at the CLI surface.
5. Same two flags on `export-snapshot` emit the same
   behavior.
6. `--max-rows-per-page 0` is rejected with a clear error
   (zero would mean "no rows per page", which is nonsense).
7. The flags are documented in `--help` output with ADR
   references.
8. Existing `partition_durability` tests remain unchanged.
9. Clippy clean.
10. 1086 workspace tests + any new §3.5 integration tests
    pass.

## 4. Alternatives considered

**A. Persistent per-partition config.** Attractive for
operators who want "every compaction of partition X uses
v1.4". Rejected: adds a config-file format question + a
persistence story + "where does the config file live"
question. ADR 0085 keeps the operator in explicit control
every time; a future ADR can add the persisted config.

**B. Add the flags on `serve` too.** Live partition writes
would then default to v1.4. Rejected: `serve` is a long-
running command where the operator rarely rewrites flags;
`compact-partition` + `export-snapshot` are the natural
"opt in once" points. Live write path is a separate ADR.

**C. Auto-select N based on row count.** Rejected for v1:
makes the knob stop being a knob, and operators who need
large pages for write throughput can't opt out. Can be
added later.

**D. Rename `--use-column-dict` to something more
discoverable.** `--v14-column-dict`? `--shared-dict`?
Defer naming bikeshed to §5.

## 5. Open design questions (owner review)

1. **Flag naming.** Draft: `--max-rows-per-page` +
   `--use-column-dict`. Alternatives: `--rows-per-page`,
   `--shared-dict`, `--v14`. Confirm.
2. **Default values.** Draft: `None` / `false` to preserve
   pre-ADR-0085 byte output. Alternative: auto-enable v1.4
   when `--max-rows-per-page` is small (e.g., ≤ 32).
   Confirm: no auto-enable.
3. **Validation of `--max-rows-per-page 0`.** Draft:
   reject with "must be > 0" error. Confirm.
4. **Help text ADR references.** Draft: include ADR numbers
   inline in `--help`. Alternative: just describe the
   behavior. Confirm the ADR references are ok.
5. **Serve command.** Draft: NOT in scope. Confirm.
6. **PartitionOptions default()**  must set the two new
   fields to `None` / `false`. Confirm no behavior change
   for in-tree callers.
7. **Integration-test placement.** Draft: a new test file
   `tests/compact_v14.rs` under `bradian-admin`. Alternative:
   extend `local_sql_cli.rs`. Confirm the new-file choice.

## 6. Impact on invariants

- **INV-005 recoverable lifecycle:** CLI surface addition;
  default behavior unchanged. v1.4 segments produced by
  this CLI are readable by `open_with_bace` per ADR 0083.

## 7. Dependencies this ADR would add

None.

## 8. What this ADR explicitly does NOT do

- Change live partition flush behavior.
- Persist the knobs per-partition.
- Auto-select values.
- Change the `serve` command.
- Introduce any format change.

## 9. Acceptance evidence required before marking Accepted

1. ADRs 0060–0084 accepted — **satisfied**.
2. Owner sign-off on §5 open design questions.
3. Implementation satisfying every item in §3.4.
4. Integration tests on the actual `bradian-db` binary
   (spawning a subprocess) confirming the two flags
   produce the expected on-disk format.
5. No regressions in any workspace test.
