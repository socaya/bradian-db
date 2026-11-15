//! `.derc` v1 reader.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use byteorder::{LittleEndian, ReadBytesExt};
use dap_core::{DapError, DapResult};

use crate::bitmap::Bitmap;
use crate::checksum::crc32_parts;
use crate::codec::{compress, dict as dict_codec, plain, utf8 as utf8_codec};
use crate::column::ColumnBuffer;

use super::format::{
    CodecId, ColumnMeta, FeatureFlags, PageMeta, SegmentFooter, SEGMENT_MAGIC,
    SEGMENT_VERSION_MAJOR,
};
use super::writer::FOOTER_MAGIC;

#[derive(Debug)]
pub struct SegmentReader {
    file: File,
    footer: SegmentFooter,
    /// ADR 0076 — present when the reader was opened via
    /// `open_with_bace`. Controls BACE envelope decode when a
    /// page's trailer byte indicates a BACE-wrapped body.
    bace_wiring: Option<bradian_relational::row_store::BaceWiring>,
    /// ADR 0076 — true if the segment header had
    /// `FeatureFlags::BACE_PAGE_ENVELOPE` set. Reader stores it
    /// so per-page reads can decide whether to look for a
    /// trailer byte without re-parsing the header.
    _bace_feature_active: bool,
    /// ADR 0079 — per-column dictionaries parsed from the v1.2
    /// dictionary pool. Empty when the segment is v1.0 or v1.1.
    bace_dictionaries: std::collections::HashMap<
        String, bradian_relational::bace::BaceDictionary,
    >,
}

impl SegmentReader {
    pub fn open(path: &Path) -> DapResult<Self> {
        let mut file = File::open(path)?;
        let (footer, bace_feature, _dict_feature) =
            read_footer_with_features(&mut file, /*allow_bace=*/false)?;
        Ok(Self {
            file, footer,
            bace_wiring: None,
            _bace_feature_active: bace_feature,
            bace_dictionaries: std::collections::HashMap::new(),
        })
    }

    /// ADR 0076 + ADR 0079 — open a `.derc` segment that may
    /// carry BACE-wrapped page bodies (v1.1) and/or per-column
    /// embedded dictionaries (v1.2). The wiring is consulted
    /// per page if the segment's `BACE_PAGE_ENVELOPE` feature
    /// flag was set; the dictionary pool is parsed when the
    /// v1.2 `BACE_PAGE_DICTIONARY` feature flag was set.
    pub fn open_with_bace(
        path: &Path,
        wiring: bradian_relational::row_store::BaceWiring,
    ) -> DapResult<Self> {
        let mut file = File::open(path)?;
        let (footer, bace_feature, dict_feature) =
            read_footer_with_features(&mut file, /*allow_bace=*/true)?;
        let bace_dictionaries = if dict_feature {
            read_dictionary_pool(&mut file)?
        } else {
            std::collections::HashMap::new()
        };
        Ok(Self {
            file, footer, bace_wiring: Some(wiring),
            _bace_feature_active: bace_feature,
            bace_dictionaries,
        })
    }

    pub fn footer(&self) -> &SegmentFooter {
        &self.footer
    }

    pub fn columns(&self) -> &[ColumnMeta] {
        &self.footer.columns
    }

    pub fn column(&self, name: &str) -> DapResult<&ColumnMeta> {
        self.footer
            .columns
            .iter()
            .find(|c| c.name == name)
            .ok_or_else(|| DapError::NotFound(format!("column {name}")))
    }

    /// Materialise the entire column as a single `ColumnBuffer`. Multi-page concatenation is
    /// handled transparently.
    pub fn read_column(&mut self, name: &str) -> DapResult<ColumnBuffer> {
        let col_idx = self
            .footer
            .columns
            .iter()
            .position(|c| c.name == name)
            .ok_or_else(|| DapError::NotFound(format!("column {name}")))?;
        let col = self.footer.columns[col_idx].clone();

        if col.pages.is_empty() {
            return Err(DapError::corruption(format!("column {name} has no pages")));
        }
        if col.pages.len() == 1 {
            return self.read_page(&col, &col.pages[0]);
        }
        // v1 writers emit single-page columns, but the reader supports multi-page concatenation.
        let mut pages = Vec::with_capacity(col.pages.len());
        for p in &col.pages {
            pages.push(self.read_page(&col, p)?);
        }
        concat_buffers(&col, pages)
    }

    fn read_page(&mut self, col: &ColumnMeta, page: &PageMeta) -> DapResult<ColumnBuffer> {
        // Read payload.
        self.file.seek(SeekFrom::Start(page.payload_offset))?;
        let mut payload = vec![0u8; page.payload_len as usize];
        self.file.read_exact(&mut payload)?;

        // Read validity bitmap if any.
        let validity_bytes = if page.validity_len > 0 {
            self.file.seek(SeekFrom::Start(page.validity_offset))?;
            let mut v = vec![0u8; page.validity_len as usize];
            self.file.read_exact(&mut v)?;
            v
        } else {
            Vec::new()
        };

        // ADR 0076 §3.3 — when the segment carries the
        // `BACE_PAGE_ENVELOPE` feature flag, one trailer byte
        // follows each page's (payload || validity). It is NOT
        // covered by the page CRC32 (preserves v1.0 byte compat);
        // its single-byte domain constrains tampering to
        // observable values and each BACE v1 envelope has its own
        // CRC+header checks.
        let bace_tag = if self._bace_feature_active {
            let trailer_offset = if page.validity_len > 0 {
                page.validity_offset + page.validity_len as u64
            } else {
                page.payload_offset + page.payload_len as u64
            };
            self.file.seek(SeekFrom::Start(trailer_offset))?;
            let mut t = [0u8; 1];
            self.file.read_exact(&mut t)?;
            t[0]
        } else {
            0
        };

        // Verify CRC before decoding.
        let expect = crc32_parts(&[&payload, &validity_bytes]);
        if expect != page.crc32 {
            return Err(DapError::corruption(format!(
                "page crc32 mismatch (column {}): expected {}, got {}",
                col.name, page.crc32, expect
            )));
        }

        // ADR 0076 §3.3 — decode path depends on the trailer byte.
        // `bace_tag == 1`: BACE v1 envelope wraps the compressed
        // inner codec output. Decode the envelope via the wiring,
        // then skip the legacy `compress::decompress` step.
        // `bace_tag == 0`: legacy path.
        // `bace_tag == 2`: BACA reserved, unsupported.
        // `bace_tag >= 3`: corruption.
        let inner = match bace_tag {
            0 => {
                if page.compressed {
                    compress::decompress(&payload, page.uncompressed_len as usize)?
                } else {
                    if payload.len() != page.uncompressed_len as usize {
                        return Err(DapError::corruption(format!(
                            "uncompressed payload length mismatch on column {}",
                            col.name
                        )));
                    }
                    payload
                }
            }
            1 => {
                let wiring = self.bace_wiring.as_ref().ok_or_else(|| {
                    DapError::unsupported(format!(
                        "page in column {} is BACE-wrapped but SegmentReader has no wiring; use SegmentReader::open_with_bace",
                        col.name,
                    ))
                })?;
                decode_bace_v1_page(&payload, page.uncompressed_len, wiring)
                    .map_err(|e| DapError::corruption(format!(
                        "BACE envelope decode failed on column {}: {e}", col.name,
                    )))?
            }
            2 => {
                // ADR 0080 §3.4 — BACA-wrapped page. Verify MAC
                // under the wiring's BaceAuthentication before
                // decoding. Fail closed if authentication is
                // absent or verification fails.
                let wiring = self.bace_wiring.as_ref().ok_or_else(|| {
                    DapError::unsupported(format!(
                        "page in column {} is BACA-authenticated but SegmentReader has no wiring; use SegmentReader::open_with_bace",
                        col.name,
                    ))
                })?;
                let auth = wiring.authentication.as_ref().ok_or_else(|| {
                    DapError::unsupported(format!(
                        "page in column {} is BACA-authenticated but BaceWiring has no authentication; supply BaceWiring::authentication",
                        col.name,
                    ))
                })?;
                decode_baca_page(&payload, page.uncompressed_len, wiring, auth)
                    .map_err(|e| DapError::corruption(format!(
                        "BACA MAC verification failed on column {} (wrong key or tampered segment): {e}",
                        col.name,
                    )))?
            }
            3 => {
                // ADR 0079 §3.3 — BACE envelope whose payload was
                // compressed against this column's embedded dict.
                let _wiring = self.bace_wiring.as_ref().ok_or_else(|| {
                    DapError::unsupported(format!(
                        "page in column {} is BACE-dict-wrapped but SegmentReader has no wiring; use SegmentReader::open_with_bace",
                        col.name,
                    ))
                })?;
                let dict = self.bace_dictionaries.get(&col.name).ok_or_else(|| {
                    DapError::corruption(format!(
                        "dictionary pool has no entry for column {} but page trailer is BaceV1WithDict",
                        col.name,
                    ))
                })?;
                decode_bace_v1_page_with_dict(&payload, page.uncompressed_len, dict)
                    .map_err(|e| DapError::corruption(format!(
                        "BACE dict-envelope decode failed on column {}: {e}", col.name,
                    )))?
            }
            other => {
                return Err(DapError::corruption(format!(
                    "page in column {} has unknown BACE envelope tag {other}",
                    col.name,
                )));
            }
        };

        // Build validity bitmap.
        let validity = if page.validity_len > 0 {
            Bitmap::from_bytes(validity_bytes, page.row_count as usize)
        } else {
            Bitmap::all_set(page.row_count as usize)
        };

        decode_page(col, page, &inner, validity)
    }
}

fn decode_page(
    col: &ColumnMeta,
    page: &PageMeta,
    bytes: &[u8],
    validity: Bitmap,
) -> DapResult<ColumnBuffer> {
    use dap_core::PhysicalType;

    match (page.codec, col.physical_type) {
        (CodecId::Plain, PhysicalType::I32) => Ok(ColumnBuffer::I32 {
            data: plain::decode_i32(bytes)?,
            validity,
        }),
        (CodecId::Plain, PhysicalType::I64) => Ok(ColumnBuffer::I64 {
            data: plain::decode_i64(bytes)?,
            validity,
        }),
        (CodecId::Plain, PhysicalType::F64) => Ok(ColumnBuffer::F64 {
            data: plain::decode_f64(bytes)?,
            validity,
        }),
        (CodecId::Plain, PhysicalType::Timestamp) => Ok(ColumnBuffer::Timestamp {
            data: plain::decode_i64(bytes)?,
            validity,
        }),
        (CodecId::Plain, PhysicalType::Decimal128) => {
            let scale = decimal_scale(col)?;
            Ok(ColumnBuffer::Decimal128 {
                data: plain::decode_decimal128(bytes)?,
                validity,
                scale,
            })
        }
        (CodecId::Plain, PhysicalType::Bool) => Ok(ColumnBuffer::Bool {
            data: plain::decode_bool(bytes)?,
            validity,
            len: page.row_count as usize,
        }),
        (CodecId::PlainUtf8, _) => {
            let (offsets, data) = utf8_codec::decode(bytes)?;
            Ok(ColumnBuffer::Utf8 {
                offsets,
                data,
                validity,
            })
        }
        (CodecId::Dictionary, _) => {
            let (indices, dict_offsets, dict_data) = dict_codec::decode(bytes)?;
            Ok(ColumnBuffer::DictIndex32 {
                indices,
                validity,
                dict_offsets,
                dict_data,
            })
        }
        (codec, phys) => Err(DapError::unsupported(format!(
            "reader v1 does not decode codec {codec:?} for physical type {phys:?}"
        ))),
    }
}

fn decimal_scale(col: &ColumnMeta) -> DapResult<u8> {
    match col.logical_type {
        dap_core::LogicalType::Decimal { scale, .. } => Ok(scale),
        other => Err(DapError::invariant(format!(
            "decimal physical type on non-decimal logical {other:?}"
        ))),
    }
}

/// ADR 0076 §3.3 — decode a BACE v1 envelope wrapping the
/// compressed inner codec output. Returns the inner codec output
/// that the legacy `decode_page` can then parse. The envelope's
/// `decoded_length` must equal the page's declared
/// `uncompressed_len` (anti-swap check).
fn decode_bace_v1_page(
    payload: &[u8],
    uncompressed_len: u32,
    wiring: &bradian_relational::row_store::BaceWiring,
) -> Result<Vec<u8>, String> {
    use bradian_relational::bace::envelope::{read_envelope, EnvelopeCaps};
    use bradian_relational::bace::policy::{PolicyKey, ScopeChain};

    let scope = ScopeChain::baseline_only();
    let cap = wiring.policy_service.resolve(PolicyKey::CompressionMaxDecodedPageBytes, &scope);
    let max_decoded = cap.value.as_u32().ok_or_else(||
        "policy service returned non-u32 for max_decoded_page_bytes".to_string(),
    )?;
    let caps = EnvelopeCaps {
        max_decoded_length: max_decoded,
        max_encoded_length: payload.len() as u32,
    };
    let env = read_envelope(payload, caps).map_err(|e| e.to_string())?;
    if env.header.decoded_length != uncompressed_len {
        return Err(format!(
            "BACE envelope decoded_length {} disagrees with page uncompressed_len {}",
            env.header.decoded_length, uncompressed_len,
        ));
    }
    let codec = wiring.registry.codec(env.header.codec_id)
        .ok_or_else(|| format!("BACE codec {:?} not registered in this build", env.header.codec_id))?;
    if codec.capability().format_version != env.header.codec_format_version {
        return Err(format!(
            "BACE codec {:?} format_version {} disagrees with build {}",
            env.header.codec_id, env.header.codec_format_version, codec.capability().format_version,
        ));
    }
    let mut out = Vec::with_capacity(uncompressed_len as usize);
    codec.decode(env.payload, uncompressed_len, &mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn concat_buffers(_col: &ColumnMeta, _pages: Vec<ColumnBuffer>) -> DapResult<ColumnBuffer> {
    Err(DapError::unsupported(
        "multi-page column concatenation not implemented in v1 (single-page writer)",
    ))
}

/// ADR 0076 + ADR 0079 — returns
/// `(footer, bace_feature_active, bace_dictionary_feature_active)`.
/// When `allow_bace` is false (legacy `SegmentReader::open`), a
/// segment that sets any BACE feature flag is rejected with the
/// existing unsupported-feature error. When `allow_bace` is true,
/// the flags are accepted and surfaced for per-page decode
/// routing + optional dictionary-pool parsing.
fn read_footer_with_features(
    file: &mut File, allow_bace: bool,
) -> DapResult<(SegmentFooter, bool, bool)> {
    let file_len = file.metadata()?.len();
    if file_len < 64 + 8 {
        return Err(DapError::corruption("file too short to be a valid .derc"));
    }
    // Verify magic.
    file.seek(SeekFrom::Start(0))?;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)?;
    if magic != SEGMENT_MAGIC {
        return Err(DapError::corruption("bad magic header"));
    }
    let major = file.read_u16::<LittleEndian>()?;
    let _minor = file.read_u16::<LittleEndian>()?;
    if major != SEGMENT_VERSION_MAJOR {
        return Err(DapError::unsupported(format!(
            "segment major version {major} not supported by v1 reader"
        )));
    }
    let required = file.read_u64::<LittleEndian>()?;
    let _optional = file.read_u64::<LittleEndian>()?;
    let subset_ok = if allow_bace {
        FeatureFlags::is_subset_supported_v1_3(required)
    } else {
        FeatureFlags::is_subset_supported(required)
    };
    if !subset_ok {
        return Err(DapError::unsupported(format!(
            "segment requires feature bits {required:#x} not supported in v1"
        )));
    }
    let bace_feature = (required & FeatureFlags::BACE_PAGE_ENVELOPE) != 0;
    let bace_dict_feature = (required & FeatureFlags::BACE_PAGE_DICTIONARY) != 0;

    // Read footer start offset (last 8 bytes).
    file.seek(SeekFrom::End(-8))?;
    let footer_start = file.read_u64::<LittleEndian>()?;
    file.seek(SeekFrom::Start(footer_start))?;

    let mut fmagic = [0u8; 4];
    file.read_exact(&mut fmagic)?;
    if fmagic != FOOTER_MAGIC {
        return Err(DapError::corruption("bad footer magic"));
    }
    let footer_len = file.read_u32::<LittleEndian>()? as usize;
    let footer_crc = file.read_u32::<LittleEndian>()?;
    let mut payload = vec![0u8; footer_len];
    file.read_exact(&mut payload)?;
    let actual_crc = crate::checksum::crc32(&payload);
    if actual_crc != footer_crc {
        return Err(DapError::corruption(format!(
            "footer crc mismatch: expected {footer_crc}, got {actual_crc}"
        )));
    }
    let footer: SegmentFooter = bincode::deserialize(&payload)
        .map_err(|e| DapError::corruption(format!("footer decode: {e}")))?;
    Ok((footer, bace_feature, bace_dict_feature))
}

/// ADR 0080 §3.4 — decode a BACA envelope (authenticated BACE
/// variant). Verifies MAC under the supplied
/// `BaceAuthentication` before decoding. Returns the inner codec
/// output (same shape as the ADR 0076 non-auth BACE decode).
fn decode_baca_page(
    payload: &[u8],
    uncompressed_len: u32,
    wiring: &bradian_relational::row_store::BaceWiring,
    auth: &bradian_relational::bace::BaceAuthentication,
) -> Result<Vec<u8>, String> {
    use bradian_relational::bace::authenticated::{
        read_authenticated_envelope, AuthenticatedEnvelopeCaps,
    };
    use bradian_relational::bace::policy::{PolicyKey, ScopeChain};

    let scope = ScopeChain::baseline_only();
    let cap = wiring.policy_service.resolve(PolicyKey::CompressionMaxDecodedPageBytes, &scope);
    let max_decoded = cap.value.as_u32().ok_or_else(|| {
        "policy service returned non-u32 for max_decoded_page_bytes".to_string()
    })?;
    let caps = AuthenticatedEnvelopeCaps {
        max_decoded_length: max_decoded,
        max_encoded_length: payload.len() as u32,
    };
    let env = read_authenticated_envelope(payload, caps, auth)
        .map_err(|e| e.to_string())?;
    if env.header.decoded_length != uncompressed_len {
        return Err(format!(
            "BACA envelope decoded_length {} disagrees with page uncompressed_len {}",
            env.header.decoded_length, uncompressed_len,
        ));
    }
    let codec = wiring.registry.codec(env.header.codec_id).ok_or_else(|| {
        format!("BACA codec {:?} not registered in this build", env.header.codec_id)
    })?;
    if codec.capability().format_version != env.header.codec_format_version {
        return Err(format!(
            "BACA codec {:?} format_version {} disagrees with build {}",
            env.header.codec_id, env.header.codec_format_version,
            codec.capability().format_version,
        ));
    }
    let mut out = Vec::with_capacity(uncompressed_len as usize);
    codec.decode(env.payload, uncompressed_len, &mut out)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// ADR 0079 §3.3 — decode a BACE envelope whose payload was
/// compressed against the supplied column dictionary. Reads the
/// envelope header, verifies `decoded_length == uncompressed_len`
/// (anti-swap check), then decodes via ADR 0078's
/// `decode_zstd_with_dictionary`.
fn decode_bace_v1_page_with_dict(
    payload: &[u8],
    uncompressed_len: u32,
    dict: &bradian_relational::bace::BaceDictionary,
) -> Result<Vec<u8>, String> {
    use bradian_relational::bace::dictionary::decode_zstd_with_dictionary;
    use bradian_relational::bace::envelope::{read_envelope, EnvelopeCaps};

    let caps = EnvelopeCaps {
        max_decoded_length: uncompressed_len,
        max_encoded_length: payload.len() as u32,
    };
    let env = read_envelope(payload, caps).map_err(|e| e.to_string())?;
    if env.header.decoded_length != uncompressed_len {
        return Err(format!(
            "BACE envelope decoded_length {} disagrees with page uncompressed_len {}",
            env.header.decoded_length, uncompressed_len,
        ));
    }
    let mut out = Vec::with_capacity(uncompressed_len as usize);
    decode_zstd_with_dictionary(env.payload, dict, uncompressed_len, &mut out)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// ADR 0079 §3.1 — parse the dictionary pool. Called only when
/// the segment header's `BACE_PAGE_DICTIONARY` feature flag was
/// set. The pool sits between the last page and FOOTER_MAGIC;
/// its length is stored in the 4 bytes immediately before
/// FOOTER_MAGIC.
fn read_dictionary_pool(
    file: &mut File,
) -> DapResult<std::collections::HashMap<
    String, bradian_relational::bace::BaceDictionary,
>> {
    // Re-read the footer start tail. (Reader has already done
    // this once, but doing it again keeps the helper
    // self-contained without threading parameters.)
    file.seek(SeekFrom::End(-8))?;
    let footer_start = file.read_u64::<LittleEndian>()?;
    // Read the u32 pool length stored immediately before
    // FOOTER_MAGIC.
    if footer_start < 4 {
        return Err(DapError::corruption("dictionary pool tail out of range"));
    }
    file.seek(SeekFrom::Start(footer_start - 4))?;
    let pool_len = file.read_u32::<LittleEndian>()? as u64;
    if pool_len > footer_start - 4 {
        return Err(DapError::corruption("dictionary pool length exceeds file"));
    }
    let pool_start = footer_start - 4 - pool_len;
    file.seek(SeekFrom::Start(pool_start))?;
    let mut buf = vec![0u8; pool_len as usize];
    file.read_exact(&mut buf)?;

    // Parse: u32 count + N × (u32 name_len + name + u32 dict_len + dict + 32-byte fingerprint)
    let mut cursor = 0usize;
    if buf.len() < 4 {
        return Err(DapError::corruption("dictionary pool truncated (no count)"));
    }
    let count = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
    cursor += 4;
    let mut pool = std::collections::HashMap::with_capacity(count);
    for _ in 0..count {
        if buf.len().saturating_sub(cursor) < 4 {
            return Err(DapError::corruption("dict pool truncated at name_len"));
        }
        let name_len = u32::from_le_bytes(buf[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        if buf.len().saturating_sub(cursor) < name_len {
            return Err(DapError::corruption("dict pool truncated at name"));
        }
        let name = std::str::from_utf8(&buf[cursor..cursor + name_len])
            .map_err(|_| DapError::corruption("dict pool name not utf-8"))?
            .to_string();
        cursor += name_len;
        if buf.len().saturating_sub(cursor) < 4 {
            return Err(DapError::corruption("dict pool truncated at dict_len"));
        }
        let dict_len = u32::from_le_bytes(buf[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        if buf.len().saturating_sub(cursor) < dict_len + 32 {
            return Err(DapError::corruption("dict pool truncated at dict+fingerprint"));
        }
        let dict_bytes = buf[cursor..cursor + dict_len].to_vec();
        cursor += dict_len;
        let declared_fp: [u8; 32] = buf[cursor..cursor + 32].try_into().unwrap();
        cursor += 32;
        // Fingerprint integrity: recompute from bytes and compare.
        let dict = bradian_relational::bace::BaceDictionary::new(
            dict_bytes, name.clone(),
        );
        if *dict.fingerprint() != declared_fp {
            return Err(DapError::corruption(format!(
                "dict pool entry for column {name}: fingerprint mismatch",
            )));
        }
        pool.insert(name, dict);
    }
    if cursor != buf.len() {
        return Err(DapError::corruption("dict pool trailing bytes"));
    }
    Ok(pool)
}
