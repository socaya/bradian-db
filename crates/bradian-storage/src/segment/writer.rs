//! `.derc` v1 writer.
//!
//! Current v1 shape: one row group per segment, one page per column. Multi-row-group splitting
//! can be added by driving the writer multiple times in a row — the format already supports
//! `Vec<PageMeta>` per column.

use std::fs::{File, OpenOptions};
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};

use byteorder::{LittleEndian, WriteBytesExt};
use bradian_core::{BradianError, BradianResult, Fingerprint, LogicalType, Schema};

use crate::checksum::crc32_parts;
use crate::codec::{compress, PageCodec};
use crate::column::{ColumnBuffer, ColumnBuilder};
use crate::zonemap::ZoneMap;

use super::format::{
    CodecId, ColumnMeta, FeatureFlags, PageMeta, SegmentFooter, SEGMENT_MAGIC,
    SEGMENT_VERSION_MAJOR, SEGMENT_VERSION_MINOR,
};

pub const FOOTER_MAGIC: [u8; 4] = *b"DRCF";

#[derive(Clone, Debug)]
pub struct SegmentWriterOptions {
    pub dataset_id: String,
    pub partition_id: String,
    pub segment_id: String,
    pub compress: bool,
    /// Prefer dictionary encoding for Utf8 columns with cardinality below this ratio.
    pub dict_cardinality_threshold: f32,
    pub watermark_ms: i64,
    pub data_revision: u64,
    pub metadata_revision: Option<Fingerprint>,
    /// ADR 0081 §3.1 — if `Some(n)`, non-dict columns are
    /// split into ceil(rows / n) pages. `None` (default)
    /// emits one page per column, preserving byte-identical
    /// pre-ADR-0081 output. Dict-encoded columns always emit
    /// one page regardless (§2 non-goal).
    pub max_rows_per_page: Option<usize>,
    /// ADR 0083 §3.3 — opt in to v1.4 column-level shared
    /// dictionary mode. When `true` and a column lands on the
    /// Dictionary codec, that column's dict bytes live once in
    /// a column-dict region between the pages and the dict
    /// pool / footer; Dictionary pages carry only indices.
    /// `false` (default) preserves pre-ADR-0083 byte output.
    pub use_column_dict: bool,
}

impl Default for SegmentWriterOptions {
    fn default() -> Self {
        Self {
            dataset_id: String::new(),
            partition_id: String::new(),
            segment_id: String::new(),
            compress: true,
            dict_cardinality_threshold: 0.5,
            watermark_ms: 0,
            data_revision: 0,
            metadata_revision: None,
            max_rows_per_page: None,
            use_column_dict: false,
        }
    }
}

pub struct SegmentWriter {
    schema: Schema,
    builders: Vec<ColumnBuilder>,
    use_dict: Vec<bool>,
    options: SegmentWriterOptions,
    rows: u64,
    /// ADR 0076 — present when the writer was constructed via
    /// `new_with_bace`. Controls whether page bodies are wrapped in
    /// a BACE envelope and whether the segment header sets the
    /// `BACE_PAGE_ENVELOPE` feature flag.
    bace_wiring: Option<bradian_relational::row_store::BaceWiring>,
    /// ADR 0079 — per-column BACE dictionaries. Columns in this
    /// map use dict-aware encode (trailer byte `3`); columns
    /// absent fall back to ADR 0076 v1.1 behavior (trailer `1`).
    /// Empty when the writer was constructed without
    /// `new_with_bace_and_dictionaries`.
    bace_dictionaries:
        std::collections::HashMap<String, bradian_relational::bace::BaceDictionary>,
}

impl SegmentWriter {
    pub fn new(schema: Schema, options: SegmentWriterOptions) -> Self {
        Self::new_inner(schema, options, None, std::collections::HashMap::new())
    }

    /// ADR 0076 — construct a writer that emits `.derc` v1.1
    /// segments with BACE-wrapped page bodies. See ADR §3.2.
    /// When the resolved codec is RAW, the writer still emits
    /// v1.0-shape bytes (no envelope, no trailer byte) to avoid
    /// paying 36 bytes of envelope overhead per page for no
    /// compression benefit.
    pub fn new_with_bace(
        schema: Schema,
        options: SegmentWriterOptions,
        wiring: bradian_relational::row_store::BaceWiring,
    ) -> Self {
        Self::new_inner(schema, options, Some(wiring), std::collections::HashMap::new())
    }

    /// ADR 0079 — construct a writer that emits `.derc` v1.2
    /// segments with per-column embedded dictionaries. Columns
    /// present in `dictionaries` encode via
    /// `encode_zstd_with_dictionary`; columns absent fall back to
    /// the v1.1 non-dict BACE path (ADR 0076). An empty map
    /// produces a v1.1-shape file (no `BACE_PAGE_DICTIONARY`
    /// feature flag, no dictionary pool, byte-identical to
    /// `new_with_bace`).
    pub fn new_with_bace_and_dictionaries(
        schema: Schema,
        options: SegmentWriterOptions,
        wiring: bradian_relational::row_store::BaceWiring,
        dictionaries: std::collections::HashMap<
            String,
            bradian_relational::bace::BaceDictionary,
        >,
    ) -> Self {
        Self::new_inner(schema, options, Some(wiring), dictionaries)
    }

    fn new_inner(
        schema: Schema,
        options: SegmentWriterOptions,
        bace_wiring: Option<bradian_relational::row_store::BaceWiring>,
        bace_dictionaries: std::collections::HashMap<
            String,
            bradian_relational::bace::BaceDictionary,
        >,
    ) -> Self {
        let mut builders = Vec::with_capacity(schema.len());
        let mut use_dict = Vec::with_capacity(schema.len());
        for c in &schema.columns {
            // We defer the dict-vs-plain choice until finish for Utf8 columns; keep two builders
            // only if explicitly requested. For v1 simplicity: use dict for Utf8/Uid/Period by
            // default (DHIS2 workloads are overwhelmingly low-cardinality on these fields) and
            // fall back to plain in `finalise_column` if cardinality says otherwise.
            match c.logical_type {
                LogicalType::Utf8 | LogicalType::Uid | LogicalType::Period => {
                    builders.push(ColumnBuilder::new_dict(c.logical_type));
                    use_dict.push(true);
                }
                _ => {
                    builders.push(ColumnBuilder::new(c.logical_type));
                    use_dict.push(false);
                }
            }
        }
        Self {
            schema,
            builders,
            use_dict,
            options,
            rows: 0,
            bace_wiring,
            bace_dictionaries,
        }
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// Append one row's cells. Caller must supply one `Value` per schema column.
    pub fn append_row(&mut self, cells: &[bradian_core::Value]) -> BradianResult<()> {
        if cells.len() != self.schema.len() {
            return Err(BradianError::schema_mismatch(format!(
                "row has {} cells, schema has {}",
                cells.len(),
                self.schema.len()
            )));
        }
        for (i, cell) in cells.iter().enumerate() {
            self.builders[i].append_value(cell)?;
        }
        self.rows += 1;
        Ok(())
    }

    /// Append many rows as columnar input. `columns[i]` must align with `schema.columns[i]`.
    pub fn append_columns(&mut self, columns: &[ColumnBuffer]) -> BradianResult<()> {
        if columns.len() != self.schema.len() {
            return Err(BradianError::schema_mismatch("column count mismatch"));
        }
        if columns.is_empty() {
            return Ok(());
        }
        let n = columns[0].len();
        for (i, buf) in columns.iter().enumerate() {
            if buf.len() != n {
                return Err(BradianError::schema_mismatch(format!(
                    "column {} has {} rows, expected {}",
                    i,
                    buf.len(),
                    n
                )));
            }
            // Fall back to row-wise append via `value_at` — acceptable at v1 writer scale.
            // Hot batch-ingest paths will lower this to typed column copies in a later gate.
            for r in 0..n {
                let v = buf.value_at(r)?;
                self.builders[i].append_value(&v)?;
            }
        }
        self.rows += n as u64;
        Ok(())
    }

    /// Finalise the segment and write it to `path`. Uses temp-and-rename for atomic publication.
    pub fn finish_to_path(self, path: &Path) -> BradianResult<WrittenSegment> {
        let tmp = tmp_path(path);
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        let mut w = match &self.bace_wiring {
            Some(wiring) => SegmentFileWriter::new_with_bace(
                file, self.options.clone(), wiring.clone(),
                self.bace_dictionaries.clone(),
            ),
            None => SegmentFileWriter::new(file, self.options.clone()),
        };

        w.write_header()?;
        let mut columns_meta = Vec::with_capacity(self.schema.len());
        for (i, builder) in self.builders.into_iter().enumerate() {
            let col = &self.schema.columns[i];
            let buf = builder.finish();
            let (meta, zone) = w.write_column(col.name.clone(), col, buf, self.use_dict[i])?;
            columns_meta.push(ColumnMeta {
                name: col.name.clone(),
                logical_type: col.logical_type,
                physical_type: col.logical_type.default_physical(),
                nullable: col.nullable,
                pages: meta,
                zone_map: zone,
            });
        }

        let fingerprint = schema_fingerprint(&self.schema);
        let footer = SegmentFooter {
            dataset_id: self.options.dataset_id.clone(),
            partition_id: self.options.partition_id.clone(),
            segment_id: self.options.segment_id.clone(),
            schema_fingerprint: fingerprint,
            metadata_revision: self.options.metadata_revision,
            row_count: self.rows,
            columns: columns_meta,
            watermark_ms: self.options.watermark_ms,
            data_revision: self.options.data_revision,
        };
        // ADR 0083 §3.3 — v1.4 column-dict region. Written before
        // the dict pool (if any) so the reader can walk regions
        // from the footer backwards.
        if w.options.use_column_dict {
            w.write_column_dict_region()?;
        }
        // ADR 0079 §3.1 — v1.2 segments embed per-column
        // dictionaries in a pool between the last page and the
        // footer. When no dicts are attached, this is a no-op and
        // the file layout is byte-identical to v1.0/v1.1.
        if w.bace_active && !w.bace_dictionaries.is_empty() {
            w.write_dictionary_pool()?;
        }
        w.write_footer(&footer)?;
        let bytes_written = w.flush_and_sync()?;

        std::fs::rename(&tmp, path)?;
        // fsync the parent directory so the rename is durable. Best-effort — some filesystems
        // (fat, some network mounts) reject dir fsync.
        if let Some(parent) = path.parent() {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }

        Ok(WrittenSegment {
            path: path.to_path_buf(),
            size_bytes: bytes_written,
            footer,
        })
    }

    pub fn row_count(&self) -> u64 {
        self.rows
    }
}

#[derive(Debug)]
pub struct WrittenSegment {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub footer: SegmentFooter,
}

fn tmp_path(final_path: &Path) -> PathBuf {
    let mut s = final_path.as_os_str().to_os_string();
    s.push(".tmp");
    PathBuf::from(s)
}

fn schema_fingerprint(schema: &Schema) -> Fingerprint {
    let json = serde_json::to_vec(&schema).expect("schema is serialisable");
    Fingerprint::of(&json)
}

/// ADR 0076 §3.2 — encode `inner_payload` under the wiring's
/// resolved outer codec and wrap it in a BACE v1 envelope. The
/// resulting bytes are what the writer places on disk as the
/// page body. The envelope's `codec_id` matches the resolved
/// codec (ZSTD=49 today).
fn wrap_page_in_bace_envelope(
    inner_payload: &[u8],
    wiring: &bradian_relational::row_store::BaceWiring,
) -> BradianResult<Vec<u8>> {
    use bradian_relational::bace::codec::raw::RAW_ID;
    use bradian_relational::bace::codec::CodecId;
    use bradian_relational::bace::envelope::{
        write_envelope, EnvelopeFlags, EnvelopeHeader,
    };
    use bradian_relational::bace::policy::{PolicyKey, ScopeChain};

    let scope = ScopeChain::baseline_only();
    let allowed = wiring.policy_service.resolve(PolicyKey::CompressionAllowedCodecs, &scope);
    let allowed = allowed.value.as_codec_set().ok_or_else(|| BradianError::corruption(
        "BACE wiring policy service returned non-codec-set for allowed_codecs",
    ))?;
    // Pick the first registered non-RAW codec.
    let mut chosen: Option<CodecId> = None;
    for id in allowed {
        if *id != RAW_ID && wiring.registry.codec(*id).is_some() {
            chosen = Some(*id);
            break;
        }
    }
    let chosen = chosen.ok_or_else(|| BradianError::corruption(
        "BACE wiring has no non-RAW codec available for .derc page wrapping",
    ))?;
    let codec = wiring.registry.codec(chosen).expect("registry lookup just succeeded");
    let mut encoded = Vec::with_capacity(inner_payload.len());
    codec.encode(inner_payload, &mut encoded).map_err(|e| BradianError::corruption(
        format!("BACE codec encode failed: {e}"),
    ))?;
    let header = EnvelopeHeader {
        codec_id: chosen,
        codec_format_version: codec.capability().format_version,
        flags: EnvelopeFlags {
            payload_is_raw: chosen == RAW_ID,
            critical: codec.capability().flags.critical,
            ..EnvelopeFlags::default()
        },
        encoded_length: encoded.len() as u32,
        decoded_length: inner_payload.len() as u32,
    };
    let mut buf = Vec::with_capacity(encoded.len() + 36);
    write_envelope(&mut buf, &header, &encoded).map_err(|e| BradianError::corruption(
        format!("BACE envelope write failed: {e}"),
    ))?;
    Ok(buf)
}

/// ADR 0080 §3.3 — encode `inner_payload` via the wiring's
/// resolved outer codec (ZSTD today), then wrap the output in a
/// BACA v1 envelope with the operator-held authentication key.
/// The MAC covers the authenticated envelope header + the
/// encoded payload (ADR 0075). `codec_id` names ZSTD so the
/// reader can decode after MAC verification.
fn wrap_page_in_baca_envelope(
    inner_payload: &[u8],
    wiring: &bradian_relational::row_store::BaceWiring,
    auth: &bradian_relational::bace::BaceAuthentication,
) -> BradianResult<Vec<u8>> {
    use bradian_relational::bace::authenticated::{
        write_authenticated_envelope, AuthenticatedEnvelopeFlags,
        AuthenticatedEnvelopeHeader,
    };
    use bradian_relational::bace::codec::raw::RAW_ID;
    use bradian_relational::bace::codec::CodecId;
    use bradian_relational::bace::policy::{PolicyKey, ScopeChain};

    let scope = ScopeChain::baseline_only();
    let allowed = wiring.policy_service.resolve(PolicyKey::CompressionAllowedCodecs, &scope);
    let allowed = allowed.value.as_codec_set().ok_or_else(|| BradianError::corruption(
        "BACE wiring policy service returned non-codec-set for allowed_codecs",
    ))?;
    let mut chosen: Option<CodecId> = None;
    for id in allowed {
        if *id != RAW_ID && wiring.registry.codec(*id).is_some() {
            chosen = Some(*id);
            break;
        }
    }
    let chosen = chosen.ok_or_else(|| BradianError::corruption(
        "BACE wiring has no non-RAW codec available for .derc BACA wrapping",
    ))?;
    let codec = wiring.registry.codec(chosen).expect("registry lookup just succeeded");
    let mut encoded = Vec::with_capacity(inner_payload.len());
    codec.encode(inner_payload, &mut encoded).map_err(|e|
        BradianError::corruption(format!("BACA codec encode failed: {e}")),
    )?;
    let header = AuthenticatedEnvelopeHeader {
        codec_id: chosen,
        codec_format_version: codec.capability().format_version,
        flags: AuthenticatedEnvelopeFlags {
            payload_is_raw: false,
            critical: codec.capability().flags.critical,
            ..Default::default()
        },
        encoded_length: encoded.len() as u32,
        decoded_length: inner_payload.len() as u32,
    };
    let mut buf = Vec::with_capacity(encoded.len() + 52);
    write_authenticated_envelope(&mut buf, &header, &encoded, auth).map_err(|e|
        BradianError::corruption(format!("BACA envelope write failed: {e}")),
    )?;
    Ok(buf)
}

/// ADR 0081 §3.2 — convert a dict-builder-produced `DictIndex32`
/// buffer into a `Utf8` buffer so the writer can slice it across
/// pages. Only called when the chosen codec is `PlainUtf8` but
/// the buffer is still in the dict-builder shape. Panics on
/// non-DictIndex32 input (the caller guards).
fn materialize_dict_to_utf8(buf: &ColumnBuffer) -> ColumnBuffer {
    let ColumnBuffer::DictIndex32 { indices, validity, dict_offsets, dict_data } = buf else {
        panic!("materialize_dict_to_utf8 requires a DictIndex32 buffer");
    };
    let mut offsets = Vec::with_capacity(indices.len() + 1);
    let mut data = Vec::new();
    offsets.push(0u32);
    for idx in indices {
        let s = dict_offsets[*idx as usize] as usize;
        let e = dict_offsets[*idx as usize + 1] as usize;
        data.extend_from_slice(&dict_data[s..e]);
        offsets.push(data.len() as u32);
    }
    ColumnBuffer::Utf8 { offsets, data, validity: validity.clone() }
}

/// ADR 0079 §3.3 — encode `inner_payload` with the supplied BACE
/// dictionary (via ADR 0078's `encode_zstd_with_dictionary`) and
/// wrap the result in a BACE v1 envelope with `codec_id = ZSTD_ID`.
/// The envelope flags are identical to the non-dict path; the
/// dict association is signalled via the per-page trailer byte
/// `BaceV1WithDict = 3`, not an envelope flag.
fn wrap_page_in_bace_envelope_with_dict(
    inner_payload: &[u8],
    _wiring: &bradian_relational::row_store::BaceWiring,
    dict: &bradian_relational::bace::BaceDictionary,
) -> BradianResult<Vec<u8>> {
    use bradian_relational::bace::codec::zstd::{ZSTD_FORMAT_VERSION, ZSTD_ID};
    use bradian_relational::bace::dictionary::encode_zstd_with_dictionary;
    use bradian_relational::bace::envelope::{
        write_envelope, EnvelopeFlags, EnvelopeHeader,
    };

    let mut encoded = Vec::with_capacity(inner_payload.len());
    encode_zstd_with_dictionary(inner_payload, dict, &mut encoded).map_err(|e|
        BradianError::corruption(format!("BACE dict encode failed: {e}")),
    )?;
    let header = EnvelopeHeader {
        codec_id: ZSTD_ID,
        codec_format_version: ZSTD_FORMAT_VERSION,
        flags: EnvelopeFlags {
            payload_is_raw: false,
            critical: false,
            ..EnvelopeFlags::default()
        },
        encoded_length: encoded.len() as u32,
        decoded_length: inner_payload.len() as u32,
    };
    let mut buf = Vec::with_capacity(encoded.len() + 36);
    write_envelope(&mut buf, &header, &encoded).map_err(|e|
        BradianError::corruption(format!("BACE envelope write failed: {e}")),
    )?;
    Ok(buf)
}

/// ADR 0076 §3.2 — decide whether the writer will wrap pages in a
/// BACE envelope. Returns `true` iff the wiring's resolved codec is
/// non-RAW. RAW opts out so the writer doesn't pay 36 bytes of
/// envelope overhead per page for no compression benefit.
fn resolve_bace_active(wiring: &bradian_relational::row_store::BaceWiring) -> bool {
    use bradian_relational::bace::codec::raw::RAW_ID;
    use bradian_relational::bace::policy::{PolicyKey, ScopeChain};
    let scope = ScopeChain::baseline_only();
    let allowed = wiring.policy_service.resolve(PolicyKey::CompressionAllowedCodecs, &scope);
    let Some(allowed) = allowed.value.as_codec_set() else { return false; };
    allowed.iter().any(|id| {
        *id != RAW_ID && wiring.registry.codec(*id).is_some()
    })
}

struct SegmentFileWriter {
    file: File,
    pos: u64,
    options: SegmentWriterOptions,
    /// ADR 0076 — present when the parent `SegmentWriter` carries a
    /// `BaceWiring`. The wiring is cloned once per segment write so
    /// `write_column` can consult it per page without reaching back
    /// to `SegmentWriter`.
    #[allow(dead_code)]
    bace_wiring: Option<bradian_relational::row_store::BaceWiring>,
    /// ADR 0076 — whether the segment header should set
    /// `BACE_PAGE_ENVELOPE` and whether per-page trailer bytes are
    /// emitted. Resolved once at writer construction from
    /// `bace_wiring` + the resolved codec. See `resolve_bace_active`.
    bace_active: bool,
    /// ADR 0079 — per-column dictionaries. Only consulted when
    /// `bace_active` is true. Columns not in the map fall back to
    /// the non-dict path (trailer `1`). Columns in the map emit
    /// trailer `3` and have an entry written into the dictionary
    /// pool before the footer.
    bace_dictionaries:
        std::collections::HashMap<String, bradian_relational::bace::BaceDictionary>,
    /// ADR 0083 §3.3 — per-column shared dictionaries collected
    /// during `write_column` when `options.use_column_dict` is
    /// set and a column lands on the Dictionary codec. Written
    /// once into the column-dict region after all pages. Map
    /// value: (dict_offsets, dict_data, entry_crc32).
    column_dicts: std::collections::BTreeMap<String, (Vec<u32>, Vec<u8>, u32)>,
}

impl SegmentFileWriter {
    fn new(file: File, options: SegmentWriterOptions) -> Self {
        Self {
            file, pos: 0, options,
            bace_wiring: None, bace_active: false,
            bace_dictionaries: std::collections::HashMap::new(),
            column_dicts: std::collections::BTreeMap::new(),
        }
    }

    /// ADR 0076 — constructor used by `SegmentWriter::new_with_bace`.
    /// `bace_active` is true iff the wiring resolves to a non-RAW
    /// codec; RAW opts out of envelope overhead per §3.2.
    /// ADR 0079 — also accepts an optional per-column
    /// `BaceDictionary` map; empty map preserves ADR 0076 v1.1
    /// behavior.
    fn new_with_bace(
        file: File,
        options: SegmentWriterOptions,
        wiring: bradian_relational::row_store::BaceWiring,
        bace_dictionaries: std::collections::HashMap<
            String,
            bradian_relational::bace::BaceDictionary,
        >,
    ) -> Self {
        let bace_active = resolve_bace_active(&wiring);
        Self {
            file, pos: 0, options,
            bace_wiring: Some(wiring),
            bace_active,
            bace_dictionaries,
            column_dicts: std::collections::BTreeMap::new(),
        }
    }

    fn write_all(&mut self, bytes: &[u8]) -> BradianResult<()> {
        self.file.write_all(bytes)?;
        self.pos += bytes.len() as u64;
        Ok(())
    }

    fn write_header(&mut self) -> BradianResult<()> {
        self.write_all(&SEGMENT_MAGIC)?;
        self.file.write_u16::<LittleEndian>(SEGMENT_VERSION_MAJOR)?;
        self.pos += 2;
        // ADR 0076 + ADR 0079 + ADR 0080 — minor version & feature
        // flags based on what the writer will actually emit:
        //   v1.0 — no BACE
        //   v1.1 — BACE no-dict no-auth
        //   v1.2 — BACE + dict (auth ignored on dict path)
        //   v1.3 — BACE + auth (no dict-aware pages)
        let v1_2 = self.bace_active && !self.bace_dictionaries.is_empty();
        let v1_3 = self.bace_active
            && !v1_2
            && self.bace_wiring.as_ref()
                .and_then(|w| w.authentication.as_ref())
                .is_some();
        // ADR 0083 — v1.4 wins over earlier minors if operator
        // opted in. Independent of BACE: column_dict is orthogonal.
        let v1_4 = self.options.use_column_dict;
        let minor = if v1_4 {
            crate::segment::format::SEGMENT_VERSION_MINOR_V1_4
        } else if v1_3 {
            crate::segment::format::SEGMENT_VERSION_MINOR_V1_3
        } else if v1_2 {
            crate::segment::format::SEGMENT_VERSION_MINOR_V1_2
        } else if self.bace_active {
            crate::segment::format::SEGMENT_VERSION_MINOR_V1_1
        } else {
            SEGMENT_VERSION_MINOR
        };
        self.file.write_u16::<LittleEndian>(minor)?;
        self.pos += 2;
        let mut required = FeatureFlags::COMPRESSED_PAGES
            | FeatureFlags::DICT_ENCODING
            | FeatureFlags::PAGE_CRC32;
        if self.bace_active {
            required |= FeatureFlags::BACE_PAGE_ENVELOPE;
        }
        if v1_2 {
            required |= FeatureFlags::BACE_PAGE_DICTIONARY;
        }
        if v1_3 {
            required |= FeatureFlags::BACE_PAGE_AUTHENTICATED;
        }
        if v1_4 {
            required |= FeatureFlags::COLUMN_DICT;
        }
        self.file.write_u64::<LittleEndian>(required)?;
        self.pos += 8;
        self.file.write_u64::<LittleEndian>(0)?; // optional flags
        self.pos += 8;
        // created_at is wall-clock ms. We don't rely on a system clock for correctness; the
        // segment watermark lives in the footer. Tolerate a clock-less test environment by
        // falling back to zero.
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.file.write_u64::<LittleEndian>(created_at)?;
        self.pos += 8;
        self.file.write_u64::<LittleEndian>(0)?; // writer_id placeholder
        self.pos += 8;
        self.write_all(&[0u8; 20])?; // reserved; header totals 64 bytes
        debug_assert_eq!(self.pos, 64);
        Ok(())
    }

    fn write_column(
        &mut self,
        name: String,
        col: &bradian_core::Column,
        buf: ColumnBuffer,
        use_dict: bool,
    ) -> BradianResult<(Vec<PageMeta>, ZoneMap)> {
        // Codec choice: dict for Utf8-family columns iff cardinality < threshold * rows AND
        // the column was built with the dict path. Otherwise fall back to plain + utf8 or
        // primitive plain.
        let codec = match (&buf, use_dict) {
            (ColumnBuffer::DictIndex32 { indices, dict_offsets, .. }, true) => {
                let n = indices.len().max(1);
                let card = (dict_offsets.len() - 1) as f32 / n as f32;
                if card <= self.options.dict_cardinality_threshold {
                    PageCodec::Dictionary
                } else {
                    PageCodec::PlainUtf8
                }
            }
            (ColumnBuffer::Utf8 { .. }, _) => PageCodec::PlainUtf8,
            _ => PageCodec::Plain,
        };

        // ADR 0081 §3.2 + ADR 0082 §3.2 — multi-page split
        // decision. Both dict-encoded and non-dict columns split
        // into pages of at most `max_rows_per_page` rows when the
        // option is set. Dict columns' per-page copies of the
        // column dict are byte-identical by construction (slice
        // clones the dict verbatim per ADR 0082 §3.1), so the
        // reader's identical-dict fast path applies.
        //
        // The Utf8-family builders produce a DictIndex32 buffer by
        // default (for low-cardinality friendly columns). When the
        // chosen codec is PlainUtf8 but the buffer is still
        // DictIndex32, materialize to Utf8 before slicing so the
        // slice path produces sliceable variants only.
        let effective_buf = if matches!(codec, PageCodec::PlainUtf8)
            && matches!(&buf, ColumnBuffer::DictIndex32 { .. })
        {
            materialize_dict_to_utf8(&buf)
        } else {
            buf
        };

        // ADR 0083 §3.3 — when the operator opted into v1.4
        // column-dict mode AND this column landed on the Dictionary
        // codec, capture the (dict_offsets, dict_data) pair ONCE
        // into column_dicts. Subsequent slices produce the same
        // bytes (ADR 0082 §3.1), so pages can be encoded as
        // indices-only.
        let column_dict_active = self.options.use_column_dict
            && matches!(codec, PageCodec::Dictionary)
            && matches!(&effective_buf, ColumnBuffer::DictIndex32 { .. });
        if column_dict_active {
            if let ColumnBuffer::DictIndex32 { dict_offsets, dict_data, .. } = &effective_buf {
                let mut offset_bytes = Vec::with_capacity(dict_offsets.len() * 4);
                for o in dict_offsets {
                    offset_bytes.extend_from_slice(&o.to_le_bytes());
                }
                let entry_crc = crc32_parts(&[&offset_bytes, dict_data]);
                self.column_dicts.insert(
                    name.clone(),
                    (dict_offsets.clone(), dict_data.clone(), entry_crc),
                );
            }
        }

        let total_rows = effective_buf.len();
        let page_size = match self.options.max_rows_per_page {
            Some(limit) if limit > 0 && limit < total_rows => limit,
            _ => total_rows.max(1),   // single page
        };

        let mut pages = Vec::new();
        let mut merged_zone = ZoneMap::empty();
        let mut row_cursor = 0usize;
        while row_cursor < total_rows {
            let slice_end = (row_cursor + page_size).min(total_rows);
            let page_buf = if page_size >= total_rows {
                effective_buf.clone()
            } else {
                effective_buf.slice(row_cursor, slice_end)
            };
            let (page_meta, page_zone) = self.emit_one_page(
                &name, col, page_buf, codec, column_dict_active,
            )?;
            merged_zone = merged_zone.merge(&page_zone);
            pages.push(page_meta);
            row_cursor = slice_end;
            if page_size >= total_rows { break; }
        }
        Ok((pages, merged_zone))
    }

    /// ADR 0081 §3.2 — emit exactly one page. All prior logic
    /// (BACE/BACA/dict envelope dispatch, CRC, trailer byte, zone
    /// map) is the same as before this ADR; the only change is
    /// the wrapping loop in `write_column` that may invoke this
    /// helper multiple times per column.
    fn emit_one_page(
        &mut self,
        name: &str,
        col: &bradian_core::Column,
        buf: ColumnBuffer,
        codec: PageCodec,
        column_dict_active: bool,
    ) -> BradianResult<(PageMeta, ZoneMap)> {
        let (inner_payload, row_count) = if column_dict_active {
            // ADR 0083 §3.3 — Dictionary page carries only indices.
            let bytes = crate::codec::dict::encode_indices_only(&buf)?;
            (bytes, buf.len() as u32)
        } else {
            encode_page(&buf, codec)?
        };
        // Writer policy: for nullable columns we always write the validity bitmap. For
        // non-nullable columns we omit it entirely (validity_len = 0). CRC must cover exactly
        // the bytes the reader will see on disk.
        let validity_bytes: Vec<u8> = if col.nullable {
            buf.validity().as_bytes().to_vec()
        } else {
            Vec::new()
        };

        // ADR 0076 + ADR 0079 + ADR 0080 — four BACE write paths:
        //   dict-equipped column → ADR 0079 trailer=3 (dict ignores auth)
        //   plain BACE + auth    → ADR 0080 trailer=2 (BACA)
        //   plain BACE no auth   → ADR 0076 trailer=1 (BACE)
        //   no BACE              → legacy v1.0 path, no trailer
        let dict_for_column = if self.bace_active {
            self.bace_dictionaries.get(name).cloned()
        } else {
            None
        };
        let wiring_opt = self.bace_wiring.as_ref();
        let (payload_bytes, compressed, trailer_tag) = if let Some(dict) = &dict_for_column {
            // ADR 0079 — dict-aware encode. §2 non-goal: dict + BACA
            // is out of scope for ADR 0080; dict pages stay trailer 3
            // even when wiring.authentication is set.
            let wrapped = wrap_page_in_bace_envelope_with_dict(
                &inner_payload,
                wiring_opt.expect("bace_active set without wiring"),
                dict,
            )?;
            (
                wrapped, true,
                Some(crate::segment::format::BaceEnvelopeTag::BaceV1WithDict as u8),
            )
        } else if self.bace_active {
            let wiring = wiring_opt.expect("bace_active set without wiring");
            if let Some(auth) = wiring.authentication.as_ref() {
                // ADR 0080 — BACA envelope wrapping the inner codec
                // output; MAC verified at read time.
                let wrapped = wrap_page_in_baca_envelope(
                    &inner_payload, wiring, auth,
                )?;
                (
                    wrapped, true,
                    Some(crate::segment::format::BaceEnvelopeTag::BacaV1 as u8),
                )
            } else {
                // ADR 0076 — unauthenticated BACE envelope.
                let wrapped = wrap_page_in_bace_envelope(&inner_payload, wiring)?;
                (
                    wrapped, true,
                    Some(crate::segment::format::BaceEnvelopeTag::BaceV1 as u8),
                )
            }
        } else if self.options.compress {
            (compress::compress(&inner_payload)?, true, None)
        } else {
            (inner_payload.clone(), false, None)
        };
        let uncompressed_len = inner_payload.len() as u32;

        let crc = crc32_parts(&[&payload_bytes, &validity_bytes]);

        let payload_offset = self.pos;
        self.write_all(&payload_bytes)?;
        let validity_offset = self.pos;
        if !validity_bytes.is_empty() {
            self.write_all(&validity_bytes)?;
        }
        // Trailer byte per ADR 0076/0079 — present only on
        // v1.1+ segments. Not covered by page CRC32 (preserves
        // v1.0 byte compat).
        if let Some(tag) = trailer_tag {
            self.write_all(&[tag])?;
        }

        let zone = compute_zone_map(&buf);
        let page = PageMeta {
            codec: codec_to_format(codec),
            compressed,
            payload_offset,
            payload_len: payload_bytes.len() as u32,
            uncompressed_len,
            validity_offset,
            validity_len: validity_bytes.len() as u32,
            row_count,
            zone_map: zone.clone(),
            crc32: crc,
        };
        let _ = col;   // reserved for future per-column decisions
        Ok((page, zone))
    }

    /// ADR 0079 §3.1 — write the dictionary pool between the last
    /// page and the footer, followed by a trailing u32 pool length
    /// so the reader can find it deterministically by seeking
    /// 4 bytes before FOOTER_MAGIC.
    ///
    /// Pool layout:
    ///   u32 entry_count
    ///   N × (u32 name_len + name_bytes
    ///       + u32 dict_len + dict_bytes + 32-byte fingerprint)
    ///
    /// After the pool, a u32 holds the pool length in bytes
    /// (NOT including the u32 itself). Reader finds the pool by
    /// seeking `footer_start - 4` for the length and
    /// `footer_start - 4 - pool_len` for the pool start.
    /// ADR 0083 §3.3 — write the column-dict region between the
    /// last page and the dict pool / footer. Followed by a u32
    /// region length so the reader finds it by seeking backwards
    /// from the dict pool / footer.
    ///
    /// Region layout:
    ///   u32 entry_count
    ///   N × (u32 name_len + name_bytes
    ///       + u32 dict_offsets_count
    ///       + u32 dict_data_len
    ///       + dict_offsets_count × u32 offset
    ///       + dict_data_bytes
    ///       + u32 entry_crc32)
    ///
    /// After the region, a u32 holds the region length in bytes
    /// (NOT including the trailing u32 itself). Writing an empty
    /// region (count=0) when `use_column_dict` is set but no
    /// column actually used Dictionary coding is intentional: the
    /// reader sees the flag and parses count=0.
    fn write_column_dict_region(&mut self) -> BradianResult<()> {
        let region_start = self.pos;
        let count = self.column_dicts.len() as u32;
        self.file.write_u32::<LittleEndian>(count)?;
        self.pos += 4;
        // Clone entries up front to release the borrow on
        // `self.column_dicts` for the mutable `write_all` calls.
        // BTreeMap already gives deterministic name ordering.
        let entries: Vec<(String, Vec<u32>, Vec<u8>, u32)> = self.column_dicts
            .iter()
            .map(|(k, (o, d, c))| (k.clone(), o.clone(), d.clone(), *c))
            .collect();
        for (name, offsets, data, entry_crc) in &entries {
            let name_bytes = name.as_bytes();
            self.file.write_u32::<LittleEndian>(name_bytes.len() as u32)?;
            self.pos += 4;
            self.write_all(name_bytes)?;
            self.file.write_u32::<LittleEndian>(offsets.len() as u32)?;
            self.pos += 4;
            self.file.write_u32::<LittleEndian>(data.len() as u32)?;
            self.pos += 4;
            for o in offsets {
                self.file.write_u32::<LittleEndian>(*o)?;
                self.pos += 4;
            }
            self.write_all(data)?;
            self.file.write_u32::<LittleEndian>(*entry_crc)?;
            self.pos += 4;
        }
        let region_len = (self.pos - region_start) as u32;
        self.file.write_u32::<LittleEndian>(region_len)?;
        self.pos += 4;
        Ok(())
    }

    fn write_dictionary_pool(&mut self) -> BradianResult<()> {
        let pool_start = self.pos;
        let count = self.bace_dictionaries.len() as u32;
        self.file.write_u32::<LittleEndian>(count)?;
        self.pos += 4;
        // Deterministic ordering by column name — makes golden-
        // byte tests reproducible across HashMap iteration orders.
        // Clone the entries up front so we're not holding a borrow
        // of `self.bace_dictionaries` while mutating `self`.
        let mut entries: Vec<(String, Vec<u8>, [u8; 32])> = self.bace_dictionaries
            .iter()
            .map(|(k, v)| (k.clone(), v.raw_bytes_for_carrier().to_vec(), *v.fingerprint()))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, dict_bytes, fingerprint) in &entries {
            let name_bytes = name.as_bytes();
            self.file.write_u32::<LittleEndian>(name_bytes.len() as u32)?;
            self.pos += 4;
            self.write_all(name_bytes)?;
            self.file.write_u32::<LittleEndian>(dict_bytes.len() as u32)?;
            self.pos += 4;
            self.write_all(dict_bytes)?;
            self.write_all(fingerprint)?;
        }
        let pool_len = (self.pos - pool_start) as u32;
        self.file.write_u32::<LittleEndian>(pool_len)?;
        self.pos += 4;
        Ok(())
    }

    fn write_footer(&mut self, footer: &SegmentFooter) -> BradianResult<()> {
        let footer_start = self.pos;
        let payload = bincode::serialize(footer)
            .map_err(|e| BradianError::Serialization(e.to_string()))?;
        let footer_len = payload.len() as u32;
        let crc = crate::checksum::crc32(&payload);

        self.write_all(&FOOTER_MAGIC)?;
        self.file.write_u32::<LittleEndian>(footer_len)?;
        self.pos += 4;
        self.file.write_u32::<LittleEndian>(crc)?;
        self.pos += 4;
        self.write_all(&payload)?;
        self.file.write_u64::<LittleEndian>(footer_start)?;
        self.pos += 8;
        Ok(())
    }

    fn flush_and_sync(mut self) -> BradianResult<u64> {
        self.file.flush()?;
        self.file.sync_all()?;
        let pos = self.file.stream_position()?;
        Ok(pos)
    }
}

fn codec_to_format(c: PageCodec) -> CodecId {
    match c {
        PageCodec::Plain => CodecId::Plain,
        PageCodec::PlainUtf8 => CodecId::PlainUtf8,
        PageCodec::Dictionary => CodecId::Dictionary,
        PageCodec::RunLength => CodecId::RunLength,
        PageCodec::Delta => CodecId::Delta,
        PageCodec::BitPacked => CodecId::BitPacked,
    }
}

fn encode_page(buf: &ColumnBuffer, codec: PageCodec) -> BradianResult<(Vec<u8>, u32)> {
    use crate::codec::{dict, plain, utf8};
    let row_count = buf.len() as u32;
    let bytes = match codec {
        PageCodec::Plain => plain::encode(buf)?,
        PageCodec::PlainUtf8 => match buf {
            ColumnBuffer::DictIndex32 { .. } => {
                // Expand dict to plain utf8 if codec selection flipped at finish time.
                let expanded = expand_dict_to_utf8(buf)?;
                utf8::encode(&expanded)?
            }
            _ => utf8::encode(buf)?,
        },
        PageCodec::Dictionary => dict::encode(buf)?,
        _ => {
            return Err(BradianError::unsupported(format!(
                "codec {codec:?} is reserved but not produced by v1 writer"
            )))
        }
    };
    Ok((bytes, row_count))
}

fn expand_dict_to_utf8(buf: &ColumnBuffer) -> BradianResult<ColumnBuffer> {
    if let ColumnBuffer::DictIndex32 {
        indices,
        validity,
        dict_offsets,
        dict_data,
    } = buf
    {
        let mut offsets = Vec::with_capacity(indices.len() + 1);
        let mut data = Vec::new();
        offsets.push(0u32);
        for (i, idx) in indices.iter().enumerate() {
            if validity.is_valid(i) {
                let s = dict_offsets[*idx as usize] as usize;
                let e = dict_offsets[*idx as usize + 1] as usize;
                data.extend_from_slice(&dict_data[s..e]);
            }
            offsets.push(data.len() as u32);
        }
        Ok(ColumnBuffer::Utf8 {
            offsets,
            data,
            validity: validity.clone(),
        })
    } else {
        Err(BradianError::invariant("expand_dict_to_utf8: not a dict buffer"))
    }
}

fn compute_zone_map(buf: &ColumnBuffer) -> ZoneMap {
    let row_count = buf.len() as u32;
    let null_count = buf.null_count() as u32;
    let (min, max) = typed_min_max(buf);
    ZoneMap {
        row_count,
        null_count,
        min,
        max,
    }
}

fn typed_min_max(buf: &ColumnBuffer) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let mut min: Option<Vec<u8>> = None;
    let mut max: Option<Vec<u8>> = None;
    let n = buf.len();
    let v = buf.validity();
    macro_rules! upd {
        ($bytes:expr) => {{
            let b: Vec<u8> = $bytes;
            if min.as_ref().map(|m| b < *m).unwrap_or(true) {
                min = Some(b.clone());
            }
            if max.as_ref().map(|m| b > *m).unwrap_or(true) {
                max = Some(b);
            }
        }};
    }
    for i in 0..n {
        if !v.is_valid(i) {
            continue;
        }
        match buf {
            ColumnBuffer::I32 { data, .. } => upd!(data[i].to_be_bytes().to_vec()),
            ColumnBuffer::I64 { data, .. } => upd!(signed_to_sortable(data[i])),
            ColumnBuffer::F64 { data, .. } => upd!(float_to_sortable(data[i])),
            ColumnBuffer::Timestamp { data, .. } => upd!(signed_to_sortable(data[i])),
            ColumnBuffer::Decimal128 { data, .. } => upd!(decimal_to_sortable(data[i])),
            ColumnBuffer::Utf8 { offsets, data, .. } => {
                let s = offsets[i] as usize;
                let e = offsets[i + 1] as usize;
                upd!(data[s..e].to_vec());
            }
            ColumnBuffer::DictIndex32 {
                indices,
                dict_offsets,
                dict_data,
                ..
            } => {
                let idx = indices[i] as usize;
                if idx < dict_offsets.len() - 1 {
                    let s = dict_offsets[idx] as usize;
                    let e = dict_offsets[idx + 1] as usize;
                    upd!(dict_data[s..e].to_vec());
                }
            }
            ColumnBuffer::Bool { data, .. } => {
                let b = (data[i / 8] >> (i % 8)) & 1;
                upd!(vec![b]);
            }
        }
    }
    (min, max)
}

/// Encode a signed integer as a big-endian, sort-order-preserving byte sequence by flipping the
/// sign bit. This lets zone-map pruning use raw lexicographic comparison without type
/// knowledge.
fn signed_to_sortable(v: i64) -> Vec<u8> {
    let flipped = (v as u64) ^ (1u64 << 63);
    flipped.to_be_bytes().to_vec()
}

fn decimal_to_sortable(v: i128) -> Vec<u8> {
    let flipped = (v as u128) ^ (1u128 << 127);
    flipped.to_be_bytes().to_vec()
}

/// Lexicographic order on IEEE-754 doubles (NaNs sort last, negatives inverted).
fn float_to_sortable(v: f64) -> Vec<u8> {
    let mut bits = v.to_bits();
    if (bits & (1u64 << 63)) != 0 {
        // negative: flip all bits
        bits ^= u64::MAX;
    } else {
        // positive / zero: flip sign bit
        bits ^= 1u64 << 63;
    }
    bits.to_be_bytes().to_vec()
}
