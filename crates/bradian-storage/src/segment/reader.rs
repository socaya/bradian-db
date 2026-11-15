//! `.brcs` v1 reader.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use byteorder::{LittleEndian, ReadBytesExt};
use bradian_core::{BradianError, BradianResult};

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
    /// ADR 0083 — per-column shared dictionaries parsed from the
    /// v1.4 column-dict region. Key: column name. Value:
    /// (dict_offsets, dict_data). Empty when the segment has no
    /// `COLUMN_DICT` feature flag.
    column_dicts: std::collections::HashMap<String, (Vec<u32>, Vec<u8>)>,
}

impl SegmentReader {
    pub fn open(path: &Path) -> BradianResult<Self> {
        let mut file = File::open(path)?;
        let (footer, bace_feature, _dict_feature, _column_dict_feature) =
            read_footer_with_features(&mut file, /*allow_bace=*/false)?;
        Ok(Self {
            file, footer,
            bace_wiring: None,
            _bace_feature_active: bace_feature,
            bace_dictionaries: std::collections::HashMap::new(),
            column_dicts: std::collections::HashMap::new(),
        })
    }

    /// ADR 0076 + ADR 0079 — open a `.brcs` segment that may
    /// carry BACE-wrapped page bodies (v1.1) and/or per-column
    /// embedded dictionaries (v1.2). The wiring is consulted
    /// per page if the segment's `BACE_PAGE_ENVELOPE` feature
    /// flag was set; the dictionary pool is parsed when the
    /// v1.2 `BACE_PAGE_DICTIONARY` feature flag was set.
    pub fn open_with_bace(
        path: &Path,
        wiring: bradian_relational::row_store::BaceWiring,
    ) -> BradianResult<Self> {
        let mut file = File::open(path)?;
        let (footer, bace_feature, dict_feature, column_dict_feature) =
            read_footer_with_features(&mut file, /*allow_bace=*/true)?;

        // Regions, in file order: [pages][column_dict][dict_pool][footer].
        // Read backwards from footer_start.
        file.seek(SeekFrom::End(-8))?;
        let footer_start = file.read_u64::<LittleEndian>()?;
        let mut cursor = footer_start;

        let bace_dictionaries = if dict_feature {
            let (pool, pool_start) = read_dictionary_pool_at(&mut file, cursor)?;
            cursor = pool_start;
            pool
        } else {
            std::collections::HashMap::new()
        };
        let column_dicts = if column_dict_feature {
            let (region, region_start) = read_column_dict_region_at(&mut file, cursor)?;
            let _ = region_start;   // cursor would continue here if more regions existed
            region
        } else {
            std::collections::HashMap::new()
        };

        Ok(Self {
            file, footer, bace_wiring: Some(wiring),
            _bace_feature_active: bace_feature,
            bace_dictionaries,
            column_dicts,
        })
    }

    pub fn footer(&self) -> &SegmentFooter {
        &self.footer
    }

    pub fn columns(&self) -> &[ColumnMeta] {
        &self.footer.columns
    }

    pub fn column(&self, name: &str) -> BradianResult<&ColumnMeta> {
        self.footer
            .columns
            .iter()
            .find(|c| c.name == name)
            .ok_or_else(|| BradianError::NotFound(format!("column {name}")))
    }

    /// ADR 0088 — streaming page iterator. Yields one
    /// `BradianResult<ColumnBuffer>` per page in declared order.
    /// Iterator stops on first error. CRC + BACE/BACA +
    /// dict-bounds checks per page are preserved (delegates to
    /// `read_page`).
    pub fn iter_pages(&mut self, name: &str) -> BradianResult<ColumnPageIterator<'_>> {
        let col = self.column(name)?.clone();
        Ok(ColumnPageIterator { reader: self, col, cursor: 0, done: false })
    }

    /// Materialise the entire column as a single `ColumnBuffer`. Multi-page concatenation is
    /// handled transparently.
    pub fn read_column(&mut self, name: &str) -> BradianResult<ColumnBuffer> {
        let col_idx = self
            .footer
            .columns
            .iter()
            .position(|c| c.name == name)
            .ok_or_else(|| BradianError::NotFound(format!("column {name}")))?;
        let col = self.footer.columns[col_idx].clone();

        if col.pages.is_empty() {
            return Err(BradianError::corruption(format!("column {name} has no pages")));
        }
        // ADR 0087 §3 — v1.4 column-dict fast path: clone the
        // column dict ONCE, decode each page as indices only,
        // concat into a single DictIndex32. Avoids ADR 0083's
        // per-page clone + ADR 0084's fast-path byte compare.
        if self.column_dicts.contains_key(&col.name)
            && col.pages.iter().all(|p| p.codec == CodecId::Dictionary)
        {
            return self.read_v14_dict_column(&col);
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

    /// ADR 0087 §3 — v1.4 single-clone fast path. Preserves CRC +
    /// index-bounds checks per page and produces a single
    /// `ColumnBuffer::DictIndex32` with one copy of the dict.
    fn read_v14_dict_column(&mut self, col: &ColumnMeta) -> BradianResult<ColumnBuffer> {
        let (shared_offsets, shared_data) = self.column_dicts
            .get(&col.name)
            .cloned()
            .ok_or_else(|| BradianError::corruption(format!(
                "read_v14_dict_column: no column-dict entry for {}", col.name,
            )))?;
        let n_dict_actual = (shared_offsets.len() - 1) as u32;

        let total_rows: usize = col.pages.iter().map(|p| p.row_count as usize).sum();
        let mut indices: Vec<u32> = Vec::with_capacity(total_rows);
        let mut validity = crate::bitmap::Bitmap::with_capacity(total_rows);

        for page in &col.pages {
            // Decode page payload exactly like read_page (CRC
            // check, BACE/BACA envelope dispatch), then use the
            // v1.4 indices-only decode.
            let ColumnBuffer::DictIndex32 { indices: page_idx, validity: page_val, .. }
                = self.read_page(col, page)? else {
                    return Err(BradianError::corruption(format!(
                        "read_v14_dict_column: page did not decode to DictIndex32 on column {}",
                        col.name,
                    )));
                };
            // read_page already bounded-checked indices against the
            // shared dict (decode_page §3.4). Still concat.
            let _ = n_dict_actual;   // bound-check was in decode_page
            indices.extend_from_slice(&page_idx);
            validity.extend_from(&page_val);
        }
        Ok(ColumnBuffer::DictIndex32 {
            indices,
            validity,
            dict_offsets: shared_offsets,
            dict_data: shared_data,
        })
    }

    fn read_page(&mut self, col: &ColumnMeta, page: &PageMeta) -> BradianResult<ColumnBuffer> {
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
            return Err(BradianError::corruption(format!(
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
        // ADR 0092 §2 — strict-authenticated mode: when the
        // wiring is strict + has auth, reject trailers 0 and 1
        // (unauthenticated BACE / legacy v1.0) in favor of
        // trailer 2 (BACA). Trailer 3 (BACE-with-dict) remains
        // accepted because it has no BACA alternative today.
        let strict_auth = self.bace_wiring.as_ref().is_some_and(|w| {
            w.strict_authenticated && w.authentication.is_some()
        });
        if strict_auth && self._bace_feature_active && (bace_tag == 0 || bace_tag == 1) {
            return Err(BradianError::corruption(format!(
                "strict mode: .brcs trailer {bace_tag} rejected on column {} under authenticated wiring (ADR 0092)",
                col.name,
            )));
        }

        let inner = match bace_tag {
            0 => {
                if page.compressed {
                    compress::decompress(&payload, page.uncompressed_len as usize)?
                } else {
                    if payload.len() != page.uncompressed_len as usize {
                        return Err(BradianError::corruption(format!(
                            "uncompressed payload length mismatch on column {}",
                            col.name
                        )));
                    }
                    payload
                }
            }
            1 => {
                let wiring = self.bace_wiring.as_ref().ok_or_else(|| {
                    BradianError::unsupported(format!(
                        "page in column {} is BACE-wrapped but SegmentReader has no wiring; use SegmentReader::open_with_bace",
                        col.name,
                    ))
                })?;
                decode_bace_v1_page(&payload, page.uncompressed_len, wiring)
                    .map_err(|e| BradianError::corruption(format!(
                        "BACE envelope decode failed on column {}: {e}", col.name,
                    )))?
            }
            2 => {
                // ADR 0080 §3.4 — BACA-wrapped page. Verify MAC
                // under the wiring's BaceAuthentication before
                // decoding. Fail closed if authentication is
                // absent or verification fails.
                let wiring = self.bace_wiring.as_ref().ok_or_else(|| {
                    BradianError::unsupported(format!(
                        "page in column {} is BACA-authenticated but SegmentReader has no wiring; use SegmentReader::open_with_bace",
                        col.name,
                    ))
                })?;
                let auth = wiring.authentication.as_ref().ok_or_else(|| {
                    BradianError::unsupported(format!(
                        "page in column {} is BACA-authenticated but BaceWiring has no authentication; supply BaceWiring::authentication",
                        col.name,
                    ))
                })?;
                decode_baca_page(&payload, page.uncompressed_len, wiring, auth)
                    .map_err(|e| BradianError::corruption(format!(
                        "BACA MAC verification failed on column {} (wrong key or tampered segment): {e}",
                        col.name,
                    )))?
            }
            3 => {
                // ADR 0079 §3.3 — BACE envelope whose payload was
                // compressed against this column's embedded dict.
                let _wiring = self.bace_wiring.as_ref().ok_or_else(|| {
                    BradianError::unsupported(format!(
                        "page in column {} is BACE-dict-wrapped but SegmentReader has no wiring; use SegmentReader::open_with_bace",
                        col.name,
                    ))
                })?;
                let dict = self.bace_dictionaries.get(&col.name).ok_or_else(|| {
                    BradianError::corruption(format!(
                        "dictionary pool has no entry for column {} but page trailer is BaceV1WithDict",
                        col.name,
                    ))
                })?;
                decode_bace_v1_page_with_dict(&payload, page.uncompressed_len, dict)
                    .map_err(|e| BradianError::corruption(format!(
                        "BACE dict-envelope decode failed on column {}: {e}", col.name,
                    )))?
            }
            other => {
                return Err(BradianError::corruption(format!(
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

        let column_dict = self.column_dicts.get(&col.name);
        decode_page(col, page, &inner, validity, column_dict)
    }
}

/// ADR 0088 — streaming page iterator. Yields one page per
/// `next()` as a standalone `ColumnBuffer`. Holds a `&mut
/// SegmentReader` so no concurrent `read_column` call is
/// possible mid-iteration (compile-time enforced).
pub struct ColumnPageIterator<'r> {
    reader: &'r mut SegmentReader,
    col: ColumnMeta,
    cursor: usize,
    done: bool,
}

impl<'r> std::fmt::Debug for ColumnPageIterator<'r> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColumnPageIterator")
            .field("column", &self.col.name)
            .field("cursor", &self.cursor)
            .field("done", &self.done)
            .field("total_pages", &self.col.pages.len())
            .finish()
    }
}

impl<'r> Iterator for ColumnPageIterator<'r> {
    type Item = BradianResult<ColumnBuffer>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.cursor >= self.col.pages.len() {
            return None;
        }
        let page = self.col.pages[self.cursor].clone();
        self.cursor += 1;
        let result = self.reader.read_page(&self.col, &page);
        if result.is_err() {
            self.done = true;
        }
        Some(result)
    }
}

fn decode_page(
    col: &ColumnMeta,
    page: &PageMeta,
    bytes: &[u8],
    validity: Bitmap,
    column_dict: Option<&(Vec<u32>, Vec<u8>)>,
) -> BradianResult<ColumnBuffer> {
    use bradian_core::PhysicalType;

    // ADR 0083 §3.4 — v1.4 Dictionary page. If the reader has a
    // column-dict entry for this column, decode the page as
    // indices-only and attach the shared dict. Cross-check the
    // page-local n_dict_of_column against the actual dict size.
    if let (CodecId::Dictionary, Some((dict_offsets, dict_data))) = (page.codec, column_dict) {
        let (indices, n_dict_declared) = dict_codec::decode_indices_only(bytes)?;
        let n_dict_actual = (dict_offsets.len() - 1) as u32;
        if n_dict_declared != n_dict_actual {
            return Err(BradianError::corruption(format!(
                "column {} v1.4 page declares n_dict={} but column-dict has {}",
                col.name, n_dict_declared, n_dict_actual,
            )));
        }
        for (i, idx) in indices.iter().enumerate() {
            if *idx != u32::MAX && *idx >= n_dict_actual {
                return Err(BradianError::corruption(format!(
                    "column {} v1.4 page row {} index {} out of range (n_dict={})",
                    col.name, i, idx, n_dict_actual,
                )));
            }
        }
        return Ok(ColumnBuffer::DictIndex32 {
            indices,
            validity,
            dict_offsets: dict_offsets.clone(),
            dict_data: dict_data.clone(),
        });
    }

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
        (codec, phys) => Err(BradianError::unsupported(format!(
            "reader v1 does not decode codec {codec:?} for physical type {phys:?}"
        ))),
    }
}

fn decimal_scale(col: &ColumnMeta) -> BradianResult<u8> {
    match col.logical_type {
        bradian_core::LogicalType::Decimal { scale, .. } => Ok(scale),
        other => Err(BradianError::invariant(format!(
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

/// ADR 0081 §3.4 + ADR 0082 §3.3 — concatenate N ColumnBuffers
/// of the same physical type into one. Dict columns use the
/// identical-dict fast path (the ADR 0082 writer invariant) or
/// fall back to Utf8 materialization if the pages carry
/// differing dicts.
fn concat_buffers(_col: &ColumnMeta, pages: Vec<ColumnBuffer>) -> BradianResult<ColumnBuffer> {
    if pages.is_empty() {
        return Err(BradianError::corruption("concat_buffers called with no pages"));
    }
    if pages.len() == 1 {
        let mut pages = pages;
        return Ok(pages.remove(0));
    }

    // Check all pages share a physical type.
    let first_physical = pages[0].physical_type();
    for (i, p) in pages.iter().enumerate().skip(1) {
        if p.physical_type() != first_physical {
            return Err(BradianError::corruption(format!(
                "concat page {i} physical_type {:?} differs from page 0 {:?}",
                p.physical_type(), first_physical,
            )));
        }
    }

    use crate::bitmap::Bitmap;
    let total: usize = pages.iter().map(|p| p.len()).sum();

    match first_physical {
        bradian_core::PhysicalType::Bool => {
            let mut data = vec![0u8; total.div_ceil(8)];
            let mut validity = Bitmap::with_capacity(total);
            let mut out_row = 0usize;
            for page in &pages {
                let ColumnBuffer::Bool { data: pdata, validity: pval, len: plen } = page else {
                    return Err(BradianError::corruption("Bool concat: non-Bool page"));
                };
                for i in 0..*plen {
                    let bit = (pdata[i / 8] >> (i % 8)) & 1;
                    data[out_row / 8] |= bit << (out_row % 8);
                    validity.push(pval.is_valid(i));
                    out_row += 1;
                }
            }
            Ok(ColumnBuffer::Bool { data, validity, len: total })
        }
        bradian_core::PhysicalType::I32 => {
            let mut data = Vec::with_capacity(total);
            let mut validity = Bitmap::with_capacity(total);
            for page in &pages {
                let ColumnBuffer::I32 { data: pd, validity: pv } = page else {
                    return Err(BradianError::corruption("I32 concat: wrong page variant"));
                };
                data.extend_from_slice(pd);
                validity.extend_from(pv);
            }
            Ok(ColumnBuffer::I32 { data, validity })
        }
        bradian_core::PhysicalType::I64 => {
            let mut data = Vec::with_capacity(total);
            let mut validity = Bitmap::with_capacity(total);
            for page in &pages {
                let ColumnBuffer::I64 { data: pd, validity: pv } = page else {
                    return Err(BradianError::corruption("I64 concat: wrong page variant"));
                };
                data.extend_from_slice(pd);
                validity.extend_from(pv);
            }
            Ok(ColumnBuffer::I64 { data, validity })
        }
        bradian_core::PhysicalType::F64 => {
            let mut data = Vec::with_capacity(total);
            let mut validity = Bitmap::with_capacity(total);
            for page in &pages {
                let ColumnBuffer::F64 { data: pd, validity: pv } = page else {
                    return Err(BradianError::corruption("F64 concat: wrong page variant"));
                };
                data.extend_from_slice(pd);
                validity.extend_from(pv);
            }
            Ok(ColumnBuffer::F64 { data, validity })
        }
        bradian_core::PhysicalType::Decimal128 => {
            let mut data = Vec::with_capacity(total);
            let mut validity = Bitmap::with_capacity(total);
            let mut shared_scale: Option<u8> = None;
            for page in &pages {
                let ColumnBuffer::Decimal128 { data: pd, validity: pv, scale } = page else {
                    return Err(BradianError::corruption("Decimal128 concat: wrong page variant"));
                };
                match shared_scale {
                    None => shared_scale = Some(*scale),
                    Some(s) if s != *scale => return Err(BradianError::corruption(
                        "Decimal128 concat: conflicting scales across pages",
                    )),
                    _ => {}
                }
                data.extend_from_slice(pd);
                validity.extend_from(pv);
            }
            Ok(ColumnBuffer::Decimal128 {
                data, validity,
                scale: shared_scale.unwrap_or(0),
            })
        }
        bradian_core::PhysicalType::Utf8 => {
            let mut offsets: Vec<u32> = Vec::with_capacity(total + 1);
            let mut data = Vec::new();
            let mut validity = Bitmap::with_capacity(total);
            offsets.push(0);
            for page in &pages {
                let ColumnBuffer::Utf8 { offsets: pofs, data: pd, validity: pv } = page else {
                    return Err(BradianError::corruption("Utf8 concat: wrong page variant"));
                };
                let base = *offsets.last().unwrap();
                // Skip the first offset (always 0) and rebase the rest.
                for o in &pofs[1..] {
                    offsets.push(base + o);
                }
                data.extend_from_slice(pd);
                validity.extend_from(pv);
            }
            Ok(ColumnBuffer::Utf8 { offsets, data, validity })
        }
        bradian_core::PhysicalType::Timestamp => {
            let mut data = Vec::with_capacity(total);
            let mut validity = Bitmap::with_capacity(total);
            for page in &pages {
                let ColumnBuffer::Timestamp { data: pd, validity: pv } = page else {
                    return Err(BradianError::corruption("Timestamp concat: wrong page variant"));
                };
                data.extend_from_slice(pd);
                validity.extend_from(pv);
            }
            Ok(ColumnBuffer::Timestamp { data, validity })
        }
        bradian_core::PhysicalType::DictIndex32 => {
            // ADR 0084 §3.2 — delegate to `concat_dict_pages`,
            // which handles the identical-dict fast path (ADR
            // 0082 §3.3 writer invariant) and the dict-union
            // slow path. The ADR 0082 Utf8 fallback is retired;
            // differing-dict pages now union into one
            // `DictIndex32` preserving the dict-encoded shape.
            ColumnBuffer::concat_dict_pages(pages)
        }
    }
}

/// ADR 0076 + ADR 0079 + ADR 0083 — returns
/// `(footer, bace_feature, dict_pool_feature, column_dict_feature)`.
/// When `allow_bace` is false (legacy `SegmentReader::open`), a
/// segment that sets any BACE/column-dict feature flag is rejected
/// with the existing unsupported-feature error. When `allow_bace`
/// is true, flags are accepted and surfaced for per-page decode
/// routing + optional dict-pool / column-dict region parsing.
fn read_footer_with_features(
    file: &mut File, allow_bace: bool,
) -> BradianResult<(SegmentFooter, bool, bool, bool)> {
    let file_len = file.metadata()?.len();
    if file_len < 64 + 8 {
        return Err(BradianError::corruption("file too short to be a valid .brcs"));
    }
    // Verify magic.
    file.seek(SeekFrom::Start(0))?;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)?;
    if magic != SEGMENT_MAGIC {
        return Err(BradianError::corruption("bad magic header"));
    }
    let major = file.read_u16::<LittleEndian>()?;
    let _minor = file.read_u16::<LittleEndian>()?;
    if major != SEGMENT_VERSION_MAJOR {
        return Err(BradianError::unsupported(format!(
            "segment major version {major} not supported by v1 reader"
        )));
    }
    let required = file.read_u64::<LittleEndian>()?;
    let _optional = file.read_u64::<LittleEndian>()?;
    let subset_ok = if allow_bace {
        FeatureFlags::is_subset_supported_v1_4(required)
    } else {
        FeatureFlags::is_subset_supported(required)
    };
    if !subset_ok {
        return Err(BradianError::unsupported(format!(
            "segment requires feature bits {required:#x} not supported in v1"
        )));
    }
    let bace_feature = (required & FeatureFlags::BACE_PAGE_ENVELOPE) != 0;
    let bace_dict_feature = (required & FeatureFlags::BACE_PAGE_DICTIONARY) != 0;
    let column_dict_feature = (required & FeatureFlags::COLUMN_DICT) != 0;

    // Read footer start offset (last 8 bytes).
    file.seek(SeekFrom::End(-8))?;
    let footer_start = file.read_u64::<LittleEndian>()?;
    file.seek(SeekFrom::Start(footer_start))?;

    let mut fmagic = [0u8; 4];
    file.read_exact(&mut fmagic)?;
    if fmagic != FOOTER_MAGIC {
        return Err(BradianError::corruption("bad footer magic"));
    }
    let footer_len = file.read_u32::<LittleEndian>()? as usize;
    let footer_crc = file.read_u32::<LittleEndian>()?;
    let mut payload = vec![0u8; footer_len];
    file.read_exact(&mut payload)?;
    let actual_crc = crate::checksum::crc32(&payload);
    if actual_crc != footer_crc {
        return Err(BradianError::corruption(format!(
            "footer crc mismatch: expected {footer_crc}, got {actual_crc}"
        )));
    }
    let footer: SegmentFooter = bincode::deserialize(&payload)
        .map_err(|e| BradianError::corruption(format!("footer decode: {e}")))?;
    Ok((footer, bace_feature, bace_dict_feature, column_dict_feature))
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
    // ADR 0093 §3.2 — rotation fallback.
    let env = match read_authenticated_envelope(payload, caps, auth) {
        Ok(env) => env,
        Err(bradian_relational::bace::authenticated::AuthenticatedEnvelopeError::MacMismatch { .. })
            if !wiring.strict_authenticated
            && wiring.previous_authentication.is_some()
        => {
            let prev = wiring.previous_authentication.as_ref().unwrap();
            read_authenticated_envelope(payload, caps, prev)
                .map_err(|e| format!("primary and previous keys both failed: {e}"))?
        }
        Err(e) => return Err(e.to_string()),
    };
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
fn read_dictionary_pool_at(
    file: &mut File,
    region_end: u64,
) -> BradianResult<(std::collections::HashMap<
    String, bradian_relational::bace::BaceDictionary,
>, u64)> {
    // Read the u32 pool length stored immediately before
    // `region_end` (the position of the next region toward
    // the end of the file; for the pool, that's FOOTER_MAGIC).
    if region_end < 4 {
        return Err(BradianError::corruption("dictionary pool tail out of range"));
    }
    file.seek(SeekFrom::Start(region_end - 4))?;
    let pool_len = file.read_u32::<LittleEndian>()? as u64;
    if pool_len > region_end - 4 {
        return Err(BradianError::corruption("dictionary pool length exceeds file"));
    }
    let pool_start = region_end - 4 - pool_len;
    file.seek(SeekFrom::Start(pool_start))?;
    let mut buf = vec![0u8; pool_len as usize];
    file.read_exact(&mut buf)?;

    // Parse: u32 count + N × (u32 name_len + name + u32 dict_len + dict + 32-byte fingerprint)
    let mut cursor = 0usize;
    if buf.len() < 4 {
        return Err(BradianError::corruption("dictionary pool truncated (no count)"));
    }
    let count = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
    cursor += 4;
    let mut pool = std::collections::HashMap::with_capacity(count);
    for _ in 0..count {
        if buf.len().saturating_sub(cursor) < 4 {
            return Err(BradianError::corruption("dict pool truncated at name_len"));
        }
        let name_len = u32::from_le_bytes(buf[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        if buf.len().saturating_sub(cursor) < name_len {
            return Err(BradianError::corruption("dict pool truncated at name"));
        }
        let name = std::str::from_utf8(&buf[cursor..cursor + name_len])
            .map_err(|_| BradianError::corruption("dict pool name not utf-8"))?
            .to_string();
        cursor += name_len;
        if buf.len().saturating_sub(cursor) < 4 {
            return Err(BradianError::corruption("dict pool truncated at dict_len"));
        }
        let dict_len = u32::from_le_bytes(buf[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        if buf.len().saturating_sub(cursor) < dict_len + 32 {
            return Err(BradianError::corruption("dict pool truncated at dict+fingerprint"));
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
            return Err(BradianError::corruption(format!(
                "dict pool entry for column {name}: fingerprint mismatch",
            )));
        }
        pool.insert(name, dict);
    }
    if cursor != buf.len() {
        return Err(BradianError::corruption("dict pool trailing bytes"));
    }
    Ok((pool, pool_start))
}

/// ADR 0083 §3.4 — parse the column-dict region. Called only when
/// the segment header's `COLUMN_DICT` feature flag was set. The
/// region sits before the dict pool (or FOOTER_MAGIC if no pool);
/// its length is stored in the 4 bytes immediately before
/// `region_end`.
type ColumnDictRegion = std::collections::HashMap<String, (Vec<u32>, Vec<u8>)>;

fn read_column_dict_region_at(
    file: &mut File,
    region_end: u64,
) -> BradianResult<(ColumnDictRegion, u64)> {
    if region_end < 4 {
        return Err(BradianError::corruption("column-dict region tail out of range"));
    }
    file.seek(SeekFrom::Start(region_end - 4))?;
    let region_len = file.read_u32::<LittleEndian>()? as u64;
    if region_len > region_end - 4 {
        return Err(BradianError::corruption("column-dict region length exceeds file"));
    }
    let region_start = region_end - 4 - region_len;
    file.seek(SeekFrom::Start(region_start))?;
    let mut buf = vec![0u8; region_len as usize];
    file.read_exact(&mut buf)?;

    let mut cursor = 0usize;
    if buf.len() < 4 {
        return Err(BradianError::corruption("column-dict region truncated (no count)"));
    }
    let count = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
    cursor += 4;
    let mut region = std::collections::HashMap::with_capacity(count);
    for _ in 0..count {
        if buf.len().saturating_sub(cursor) < 4 {
            return Err(BradianError::corruption("column-dict truncated at name_len"));
        }
        let name_len = u32::from_le_bytes(buf[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        if buf.len().saturating_sub(cursor) < name_len {
            return Err(BradianError::corruption("column-dict truncated at name"));
        }
        let name = std::str::from_utf8(&buf[cursor..cursor + name_len])
            .map_err(|_| BradianError::corruption("column-dict name not utf-8"))?
            .to_string();
        cursor += name_len;
        if buf.len().saturating_sub(cursor) < 8 {
            return Err(BradianError::corruption("column-dict truncated at sizes"));
        }
        let offsets_count = u32::from_le_bytes(
            buf[cursor..cursor + 4].try_into().unwrap(),
        ) as usize;
        cursor += 4;
        let data_len = u32::from_le_bytes(
            buf[cursor..cursor + 4].try_into().unwrap(),
        ) as usize;
        cursor += 4;
        let entry_body_start = cursor;
        if buf.len().saturating_sub(cursor) < offsets_count * 4 + data_len + 4 {
            return Err(BradianError::corruption("column-dict truncated at body"));
        }
        let mut dict_offsets = Vec::with_capacity(offsets_count);
        for _ in 0..offsets_count {
            let o = u32::from_le_bytes(buf[cursor..cursor + 4].try_into().unwrap());
            dict_offsets.push(o);
            cursor += 4;
        }
        let dict_data = buf[cursor..cursor + data_len].to_vec();
        cursor += data_len;
        let declared_crc = u32::from_le_bytes(buf[cursor..cursor + 4].try_into().unwrap());
        cursor += 4;
        let entry_body_end = cursor - 4;   // exclude the trailing u32 crc
        let actual_crc = crc32_parts(&[&buf[entry_body_start..entry_body_end]]);
        if actual_crc != declared_crc {
            return Err(BradianError::corruption(format!(
                "column-dict entry for column {name}: crc mismatch \
                 (expected {declared_crc}, got {actual_crc})",
            )));
        }
        if dict_data.len()
            != *dict_offsets.last().unwrap_or(&0) as usize
        {
            return Err(BradianError::corruption(format!(
                "column-dict entry for column {name}: last offset does not match data len",
            )));
        }
        region.insert(name, (dict_offsets, dict_data));
    }
    if cursor != buf.len() {
        return Err(BradianError::corruption("column-dict region trailing bytes"));
    }
    Ok((region, region_start))
}
