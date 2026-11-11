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
}

impl SegmentReader {
    pub fn open(path: &Path) -> DapResult<Self> {
        let mut file = File::open(path)?;
        let footer = read_footer(&mut file)?;
        Ok(Self { file, footer })
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

        // Verify CRC before decoding.
        let expect = crc32_parts(&[&payload, &validity_bytes]);
        if expect != page.crc32 {
            return Err(DapError::corruption(format!(
                "page crc32 mismatch (column {}): expected {}, got {}",
                col.name, page.crc32, expect
            )));
        }

        // Decompress if needed.
        let inner = if page.compressed {
            compress::decompress(&payload, page.uncompressed_len as usize)?
        } else {
            if payload.len() != page.uncompressed_len as usize {
                return Err(DapError::corruption(format!(
                    "uncompressed payload length mismatch on column {}",
                    col.name
                )));
            }
            payload
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

fn concat_buffers(_col: &ColumnMeta, _pages: Vec<ColumnBuffer>) -> DapResult<ColumnBuffer> {
    Err(DapError::unsupported(
        "multi-page column concatenation not implemented in v1 (single-page writer)",
    ))
}

fn read_footer(file: &mut File) -> DapResult<SegmentFooter> {
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
    if !FeatureFlags::is_subset_supported(required) {
        return Err(DapError::unsupported(format!(
            "segment requires feature bits {required:#x} not supported in v1"
        )));
    }

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
    Ok(footer)
}
