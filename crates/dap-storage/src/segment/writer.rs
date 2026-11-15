//! `.derc` v1 writer.
//!
//! Current v1 shape: one row group per segment, one page per column. Multi-row-group splitting
//! can be added by driving the writer multiple times in a row — the format already supports
//! `Vec<PageMeta>` per column.

use std::fs::{File, OpenOptions};
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};

use byteorder::{LittleEndian, WriteBytesExt};
use dap_core::{DapError, DapResult, Fingerprint, LogicalType, Schema};

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
}

impl SegmentWriter {
    pub fn new(schema: Schema, options: SegmentWriterOptions) -> Self {
        Self::new_inner(schema, options, None)
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
        Self::new_inner(schema, options, Some(wiring))
    }

    fn new_inner(
        schema: Schema,
        options: SegmentWriterOptions,
        bace_wiring: Option<bradian_relational::row_store::BaceWiring>,
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
        }
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// Append one row's cells. Caller must supply one `Value` per schema column.
    pub fn append_row(&mut self, cells: &[dap_core::Value]) -> DapResult<()> {
        if cells.len() != self.schema.len() {
            return Err(DapError::schema_mismatch(format!(
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
    pub fn append_columns(&mut self, columns: &[ColumnBuffer]) -> DapResult<()> {
        if columns.len() != self.schema.len() {
            return Err(DapError::schema_mismatch("column count mismatch"));
        }
        if columns.is_empty() {
            return Ok(());
        }
        let n = columns[0].len();
        for (i, buf) in columns.iter().enumerate() {
            if buf.len() != n {
                return Err(DapError::schema_mismatch(format!(
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
    pub fn finish_to_path(self, path: &Path) -> DapResult<WrittenSegment> {
        let tmp = tmp_path(path);
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        let mut w = match &self.bace_wiring {
            Some(wiring) => SegmentFileWriter::new_with_bace(
                file, self.options.clone(), wiring.clone(),
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
) -> DapResult<Vec<u8>> {
    use bradian_relational::bace::codec::raw::RAW_ID;
    use bradian_relational::bace::codec::CodecId;
    use bradian_relational::bace::envelope::{
        write_envelope, EnvelopeFlags, EnvelopeHeader,
    };
    use bradian_relational::bace::policy::{PolicyKey, ScopeChain};

    let scope = ScopeChain::baseline_only();
    let allowed = wiring.policy_service.resolve(PolicyKey::CompressionAllowedCodecs, &scope);
    let allowed = allowed.value.as_codec_set().ok_or_else(|| DapError::corruption(
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
    let chosen = chosen.ok_or_else(|| DapError::corruption(
        "BACE wiring has no non-RAW codec available for .derc page wrapping",
    ))?;
    let codec = wiring.registry.codec(chosen).expect("registry lookup just succeeded");
    let mut encoded = Vec::with_capacity(inner_payload.len());
    codec.encode(inner_payload, &mut encoded).map_err(|e| DapError::corruption(
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
    write_envelope(&mut buf, &header, &encoded).map_err(|e| DapError::corruption(
        format!("BACE envelope write failed: {e}"),
    ))?;
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
}

impl SegmentFileWriter {
    fn new(file: File, options: SegmentWriterOptions) -> Self {
        Self {
            file, pos: 0, options,
            bace_wiring: None, bace_active: false,
        }
    }

    /// ADR 0076 — constructor used by `SegmentWriter::new_with_bace`.
    /// `bace_active` is true iff the wiring resolves to a non-RAW
    /// codec; RAW opts out of envelope overhead per §3.2.
    fn new_with_bace(
        file: File,
        options: SegmentWriterOptions,
        wiring: bradian_relational::row_store::BaceWiring,
    ) -> Self {
        let bace_active = resolve_bace_active(&wiring);
        Self {
            file, pos: 0, options,
            bace_wiring: Some(wiring),
            bace_active,
        }
    }

    fn write_all(&mut self, bytes: &[u8]) -> DapResult<()> {
        self.file.write_all(bytes)?;
        self.pos += bytes.len() as u64;
        Ok(())
    }

    fn write_header(&mut self) -> DapResult<()> {
        self.write_all(&SEGMENT_MAGIC)?;
        self.file.write_u16::<LittleEndian>(SEGMENT_VERSION_MAJOR)?;
        self.pos += 2;
        // ADR 0076 — emit v1.1 when the writer is going to emit at
        // least one BACE-wrapped page; v1.0 otherwise for byte-
        // identical legacy output.
        let minor = if self.bace_active {
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
        _name: String,
        col: &dap_core::Column,
        buf: ColumnBuffer,
        use_dict: bool,
    ) -> DapResult<(Vec<PageMeta>, ZoneMap)> {
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

        let (inner_payload, row_count) = encode_page(&buf, codec)?;
        // Writer policy: for nullable columns we always write the validity bitmap. For
        // non-nullable columns we omit it entirely (validity_len = 0). CRC must cover exactly
        // the bytes the reader will see on disk.
        let validity_bytes: Vec<u8> = if col.nullable {
            buf.validity().as_bytes().to_vec()
        } else {
            Vec::new()
        };

        // ADR 0076 §3.2 — when BACE wiring is active, wrap the
        // compressed inner codec output in a BACE envelope using
        // the resolved outer codec (ZSTD today; RAW opted out
        // via resolve_bace_active). The envelope layer encodes
        // the payload itself, so we hand it the raw inner bytes;
        // the legacy `compress::compress` path is used only when
        // BACE is not active.
        let (payload_bytes, compressed) = if self.bace_active {
            let wrapped = wrap_page_in_bace_envelope(
                &inner_payload,
                self.bace_wiring.as_ref().expect("bace_active set without wiring"),
            )?;
            (wrapped, true)
        } else if self.options.compress {
            (compress::compress(&inner_payload)?, true)
        } else {
            (inner_payload.clone(), false)
        };
        let uncompressed_len = inner_payload.len() as u32;

        let crc = crc32_parts(&[&payload_bytes, &validity_bytes]);

        let payload_offset = self.pos;
        self.write_all(&payload_bytes)?;
        let validity_offset = self.pos;
        if !validity_bytes.is_empty() {
            self.write_all(&validity_bytes)?;
        }
        // ADR 0076 §3.2 — per-page trailer byte for v1.1 segments
        // (gated by `bace_active`). Not covered by the page CRC32
        // (that would break v1.0 byte compatibility); its single-
        // byte domain restricts tampering to visible values.
        if self.bace_active {
            self.write_all(&[crate::segment::format::BaceEnvelopeTag::BaceV1 as u8])?;
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
        Ok((vec![page], zone))
    }

    fn write_footer(&mut self, footer: &SegmentFooter) -> DapResult<()> {
        let footer_start = self.pos;
        let payload = bincode::serialize(footer)
            .map_err(|e| DapError::Serialization(e.to_string()))?;
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

    fn flush_and_sync(mut self) -> DapResult<u64> {
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

fn encode_page(buf: &ColumnBuffer, codec: PageCodec) -> DapResult<(Vec<u8>, u32)> {
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
            return Err(DapError::unsupported(format!(
                "codec {codec:?} is reserved but not produced by v1 writer"
            )))
        }
    };
    Ok((bytes, row_count))
}

fn expand_dict_to_utf8(buf: &ColumnBuffer) -> DapResult<ColumnBuffer> {
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
        Err(DapError::invariant("expand_dict_to_utf8: not a dict buffer"))
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
