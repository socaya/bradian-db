//! Portable physical page encoding for the future Bradian row store.
//!
//! This format is independent of Rust struct layout and host endianness. It is
//! deliberately a *page codec*, not a storage manager, WAL, buffer pool, B+tree,
//! transaction engine, or a claim of crash durability.
//!
//! Layout (all multi-byte integers little-endian):
//!   [0..4]   magic "BRPG"
//!   [4..6]   format major version (u16)
//!   [6]      page kind (u8)
//!   [7]      reserved flags (must be zero)
//!   [8..16]  page identity (u64)
//!   [16..24] last-log-sequence number (u64; informational until WAL integration)
//!   [24..28] payload length (u32)
//!   [28..32] CRC32 over the whole 4096-byte page, with this field zeroed
//!   [32..]   payload and zero-filled tail
//!
//! The checksum detects accidental corruption. It is NOT authentication or
//! encryption; those controls must be implemented in a separate security layer.

use crc32fast::Hasher;
use thiserror::Error;

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_HEADER_SIZE: usize = 32;
pub const MAX_PAYLOAD_SIZE: usize = PAGE_SIZE - PAGE_HEADER_SIZE;
pub const PAGE_VERSION: u16 = 1;
const MAGIC: [u8; 4] = *b"BRPG";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PageKind {
    Data = 1,
    Index = 2,
    FreeMap = 3,
    Metadata = 4,
}

impl TryFrom<u8> for PageKind {
    type Error = PageError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Data),
            2 => Ok(Self::Index),
            3 => Ok(Self::FreeMap),
            4 => Ok(Self::Metadata),
            _ => Err(PageError::UnknownKind(value)),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PageError {
    #[error("invalid physical page length: {0}")]
    InvalidLength(usize),
    #[error("unsupported or damaged physical page magic")]
    InvalidMagic,
    #[error("unsupported page version: {0}")]
    UnsupportedVersion(u16),
    #[error("unknown page kind: {0}")]
    UnknownKind(u8),
    #[error("unsupported page flags: {0}")]
    UnsupportedFlags(u8),
    #[error("payload length exceeds a page: {0}")]
    InvalidPayloadLength(usize),
    #[error("page checksum mismatch")]
    ChecksumMismatch,
    #[error("nonzero bytes after payload")]
    NonZeroPadding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub page_id: u64,
    pub lsn: u64,
    pub kind: PageKind,
    pub payload: Vec<u8>,
}

impl Page {
    pub fn new(
        page_id: u64,
        lsn: u64,
        kind: PageKind,
        payload: Vec<u8>,
    ) -> Result<Self, PageError> {
        if payload.len() > MAX_PAYLOAD_SIZE {
            return Err(PageError::InvalidPayloadLength(payload.len()));
        }
        Ok(Self {
            page_id,
            lsn,
            kind,
            payload,
        })
    }

    /// Encode into exactly one portable 4096-byte physical page.
    pub fn encode(&self) -> Result<[u8; PAGE_SIZE], PageError> {
        if self.payload.len() > MAX_PAYLOAD_SIZE {
            return Err(PageError::InvalidPayloadLength(self.payload.len()));
        }
        let mut bytes = [0_u8; PAGE_SIZE];
        bytes[0..4].copy_from_slice(&MAGIC);
        bytes[4..6].copy_from_slice(&PAGE_VERSION.to_le_bytes());
        bytes[6] = self.kind as u8;
        // Version 1 reserves flags and requires them to remain zero.
        bytes[8..16].copy_from_slice(&self.page_id.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.lsn.to_le_bytes());
        bytes[24..28].copy_from_slice(&(self.payload.len() as u32).to_le_bytes());
        bytes[PAGE_HEADER_SIZE..PAGE_HEADER_SIZE + self.payload.len()]
            .copy_from_slice(&self.payload);
        let checksum = checksum(&bytes);
        bytes[28..32].copy_from_slice(&checksum.to_le_bytes());
        Ok(bytes)
    }

    /// Validate the complete page before exposing any payload bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, PageError> {
        if bytes.len() != PAGE_SIZE {
            return Err(PageError::InvalidLength(bytes.len()));
        }
        if bytes[0..4] != MAGIC {
            return Err(PageError::InvalidMagic);
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != PAGE_VERSION {
            return Err(PageError::UnsupportedVersion(version));
        }
        let kind = PageKind::try_from(bytes[6])?;
        if bytes[7] != 0 {
            return Err(PageError::UnsupportedFlags(bytes[7]));
        }
        let payload_len =
            u32::from_le_bytes(bytes[24..28].try_into().expect("fixed header")) as usize;
        if payload_len > MAX_PAYLOAD_SIZE {
            return Err(PageError::InvalidPayloadLength(payload_len));
        }
        let expected =
            u32::from_le_bytes(bytes[28..32].try_into().expect("fixed header"));
        if expected != checksum(bytes) {
            return Err(PageError::ChecksumMismatch);
        }
        if bytes[PAGE_HEADER_SIZE + payload_len..]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(PageError::NonZeroPadding);
        }
        Ok(Self {
            page_id: u64::from_le_bytes(bytes[8..16].try_into().expect("fixed header")),
            lsn: u64::from_le_bytes(bytes[16..24].try_into().expect("fixed header")),
            kind,
            payload: bytes[PAGE_HEADER_SIZE..PAGE_HEADER_SIZE + payload_len].to_vec(),
        })
    }
}

fn checksum(page: &[u8]) -> u32 {
    let mut hasher = Hasher::new();
    hasher.update(&page[..28]);
    hasher.update(&[0_u8; 4]);
    hasher.update(&page[32..]);
    hasher.finalize()
}
