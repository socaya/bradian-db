//! CRC32 wrappers (iSCSI polynomial via `crc32fast`). Used for per-page and footer integrity.

pub fn crc32(bytes: &[u8]) -> u32 {
    let mut h = crc32fast::Hasher::new();
    h.update(bytes);
    h.finalize()
}

pub fn crc32_parts(parts: &[&[u8]]) -> u32 {
    let mut h = crc32fast::Hasher::new();
    for p in parts {
        h.update(p);
    }
    h.finalize()
}
