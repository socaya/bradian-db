# ADR 0133 — Row ↔ column batch converter (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch E E3 primitive.
E1 (committed columnar projection) remains the last
Batch E follow-up.

## 1. Scope

New module `bradian_relational::column_batch`:

- `RowBatch { columns: Vec<(String, DataType)>, rows:
  Vec<Vec<Value>> }`.
- `ColumnBatch { name, dtype, values: Vec<Value> }`.
- `rows_to_columns(&RowBatch) -> Result<Vec<ColumnBatch>>`.
- `columns_to_rows(&[ColumnBatch]) -> Result<RowBatch>`.
- `BatchError { RowWidthMismatch / ColumnHeightMismatch /
  EmptyColumnList / DuplicateColumnName }`.

Pure data transformation. Round-trips bit-exact.
`Value::Null` is preserved in the column buffer at the
same position; no dense null bitmap yet.

## 2. Out of scope (follow-ups)

- **Engine-level vectorized execution** — SQL engines
  still return row-oriented `Vec<Vec<Value>>`. Wiring
  per-engine `scan_column_batches()` is per-engine
  ADR work.
- **SIMD / arithmetic over column buffers** — this
  primitive is layout-only.
- **Null bitmap compaction** — nulls live inline. A
  future ADR can add `BitVec` + packed value buffer
  per dtype.
- **Dictionary encoding / RLE / segment file writer**
  — ADR 0082 covers DictIndex32 segments. This
  uncompressed primitive is the stepping stone.
- **Arrow / Parquet interop** — out of scope.
- **Streaming / chunked emission** — the primitive
  operates on a complete batch; iterator-style
  streaming is a follow-up.

## 3. Compliance

- rows→columns preserves dtype and order.
- columns→rows reconstructs identical batch.
- Round-trip is bit-exact.
- Empty row list is a valid batch; columns have zero
  values.
- Row width mismatch is rejected.
- Column height mismatch is rejected.
- Empty column list is rejected both directions.
- Duplicate column names rejected.
- `height()` / `width()` accessors.
- NULL values preserved across the transform.
- 10 new tests in `column_batch::tests`.

## 4. Honest status

Foundation only. **BRD-HTAP-003 vectorized columnar
reads** is not yet satisfied because no engine emits
batches in column form; every scan path still
allocates `Vec<Vec<Value>>` and callers would re-
allocate to get columns. The primitive is the
prerequisite that future per-engine wiring consumes.
