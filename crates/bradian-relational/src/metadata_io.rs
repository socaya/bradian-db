//! Bounded metadata input and canonical serialization; never trusts a file length.
use std::fs::File;
use std::io::{self,Read,Write};
use std::path::Path;
use serde::Serialize;
use crate::row_store::{StoreError,StoreResult};
use crate::runtime_limits::{MetadataLimits,ResourceError};

#[derive(Debug)]
pub(crate) enum FixedError { Io(io::Error), Length }
fn read_retry(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    loop { match reader.read(buf) { Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue, other=>return other } }
}
fn fixed_reader<const N:usize>(mut reader: impl Read)->Result<[u8;N],FixedError> {
    let mut bytes=[0;N];
    reader.read_exact(&mut bytes).map_err(|e|if e.kind()==io::ErrorKind::UnexpectedEof { FixedError::Length } else { FixedError::Io(e) })?;
    if read_retry(&mut reader,&mut [0;1]).map_err(FixedError::Io)?!=0 { return Err(FixedError::Length); }
    Ok(bytes)
}
pub(crate) fn read_fixed<const N:usize>(path:&Path)->Result<[u8;N],FixedError> {
    fixed_reader(File::open(path).map_err(FixedError::Io)?)
}
fn exceeded(limits:MetadataLimits)->StoreError {
    ResourceError::MetadataLimit { limit:limits.max_manifest_bytes }.into()
}
fn bounded_reader(mut reader:impl Read,limits:MetadataLimits)->StoreResult<Vec<u8>> {
    let mut bytes=Vec::new(); let mut chunk=[0;8192];
    loop {
        let remaining=limits.max_manifest_bytes-bytes.len();
        if remaining==0 {
            if read_retry(&mut reader,&mut [0;1])?!=0 { return Err(exceeded(limits)); }
            return Ok(bytes);
        }
        let available=remaining.min(chunk.len());
        let n=read_retry(&mut reader,&mut chunk[..available])?;
        if n==0 { return Ok(bytes); }
        bytes.extend_from_slice(&chunk[..n]);
    }
}
pub(crate) fn read_json_bytes(path:&Path,limits:MetadataLimits)->StoreResult<Vec<u8>> {
    bounded_reader(File::open(path)?,limits)
}
struct LimitedWriter<W> { inner:W, used:usize, limits:MetadataLimits, exceeded:bool }
impl<W:Write> Write for LimitedWriter<W> {
    fn write(&mut self,bytes:&[u8])->io::Result<usize> {
        if self.used.checked_add(bytes.len()).is_none_or(|n|n>self.limits.max_manifest_bytes) {
            self.exceeded=true; return Err(io::Error::other("manifest byte limit exceeded"));
        }
        let n=self.inner.write(bytes)?; self.used+=n; Ok(n)
    }
    fn flush(&mut self)->io::Result<()> { self.inner.flush() }
}
fn serialize<T:Serialize,W:Write>(value:&T,inner:W,limits:MetadataLimits)->StoreResult<W> {
    let mut writer=LimitedWriter { inner,used:0,limits,exceeded:false };
    let result=serde_json::to_writer(&mut writer,value);
    if writer.exceeded { return Err(exceeded(limits)); }
    result.map_err(|e|StoreError::Corruption(format!("metadata serialization failed: {e}")))?;
    Ok(writer.inner)
}
pub(crate) fn json_bytes<T:Serialize>(value:&T,limits:MetadataLimits)->StoreResult<Vec<u8>> {
    serialize(value,Vec::new(),limits)
}
struct Digest(crc32fast::Hasher);
impl Write for Digest {
    fn write(&mut self,bytes:&[u8])->io::Result<usize> { self.0.update(bytes); Ok(bytes.len()) }
    fn flush(&mut self)->io::Result<()> { Ok(()) }
}
pub(crate) fn json_crc<T:Serialize>(value:&T,limits:MetadataLimits)->StoreResult<u32> {
    Ok(serialize(value,Digest(crc32fast::Hasher::new()),limits)?.0.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_records_require_exact_length_and_probe_only_one_trailing_byte() {
        assert_eq!(fixed_reader::<3>(&b"abc"[..]).unwrap(),*b"abc");
        assert!(matches!(fixed_reader::<3>(&b"ab"[..]),Err(FixedError::Length)));
        let mut input=&b"abcdef"[..];
        assert!(matches!(fixed_reader::<3>(&mut input),Err(FixedError::Length)));
        assert_eq!(input,b"ef");
    }
    #[test]
    fn bounded_input_consumes_at_most_limit_plus_one_without_trusting_length() {
        let mut input=&b"abcdefgh"[..];
        assert!(matches!(bounded_reader(&mut input,MetadataLimits { max_manifest_bytes:3 }),Err(StoreError::Resource(ResourceError::MetadataLimit { limit:3 }))));
        assert_eq!(input,b"efgh");
        assert!(bounded_reader(&b""[..],MetadataLimits { max_manifest_bytes:0 }).unwrap().is_empty());
        assert_eq!(bounded_reader(&b"abc"[..],MetadataLimits { max_manifest_bytes:3 }).unwrap(),b"abc");
        assert_eq!(bounded_reader(&b"abc"[..],MetadataLimits { max_manifest_bytes:usize::MAX }).unwrap(),b"abc");
    }
    struct Fragmented<'a> { input:&'a [u8], interrupt:bool }
    impl Read for Fragmented<'_> {
        fn read(&mut self,out:&mut [u8])->io::Result<usize> {
            if self.interrupt { self.interrupt=false; return Err(io::ErrorKind::Interrupted.into()); }
            self.interrupt=true; let n=out.len().min(1).min(self.input.len()); out[..n].copy_from_slice(&self.input[..n]); self.input=&self.input[n..]; Ok(n)
        }
    }
    #[test]
    fn interrupted_short_reads_preserve_fixed_and_bounded_contracts() {
        let fragmented=||Fragmented { input:b"abc",interrupt:true };
        assert_eq!(fixed_reader::<3>(fragmented()).unwrap(),*b"abc");
        assert_eq!(bounded_reader(fragmented(),MetadataLimits { max_manifest_bytes:3 }).unwrap(),b"abc");
    }
    #[test]
    fn bounded_serialization_and_streaming_crc_match_existing_canonical_encoding() {
        let value=vec!["escaped\n\"\\", "é", "longer text"];
        let old=serde_json::to_vec(&value).unwrap(); let limits=MetadataLimits { max_manifest_bytes:old.len() };
        assert_eq!(json_bytes(&value,limits).unwrap(),old); assert_eq!(json_crc(&value,limits).unwrap(),crc32fast::hash(&old));
        for result in [json_bytes(&value,MetadataLimits { max_manifest_bytes:old.len()-1 }).map(|_|()),json_crc(&value,MetadataLimits { max_manifest_bytes:old.len()-1 }).map(|_|())] {
            assert!(matches!(result,Err(StoreError::Resource(ResourceError::MetadataLimit { .. }))));
        }
    }
    #[test]
    fn writer_refuses_before_extending_buffer_and_checks_counter_overflow() {
        let mut writer=LimitedWriter { inner:Vec::new(),used:0,limits:MetadataLimits { max_manifest_bytes:3 },exceeded:false };
        writer.write_all(b"abc").unwrap(); assert!(writer.write_all(b"d").is_err()); assert_eq!(writer.inner,b"abc");
        let mut writer=LimitedWriter { inner:Vec::new(),used:usize::MAX,limits:MetadataLimits { max_manifest_bytes:usize::MAX },exceeded:false };
        assert!(writer.write_all(b"a").is_err()); assert!(writer.exceeded); assert!(writer.inner.is_empty());
    }
}
