//! ADR 0140 — chunked response stream primitive.
//!
//! BRD-WIRE-002 "streaming row delivery" is OPEN. ADR 0137
//! caps a single wire frame at MAX_PAYLOAD (16 MiB). For
//! result sets larger than that, this module ships the
//! fragmentation + reassembly primitive a future streaming
//! driver path can use.
//!
//! Protocol:
//! - `split(payload, chunk_size) -> Vec<Chunk>` produces
//!   N `Chunk { stream_id, seq, total, body }` values.
//! - `Chunk` serializes via serde; a future caller wraps
//!   each in a Response wire frame.
//! - `Assembler::push(chunk)` accumulates by (`stream_id`,
//!   `seq`). When every seq in `[0, total)` is present,
//!   `Assembler::take()` yields the concatenated payload.
//! - `Assembler` rejects mismatched `total` / `stream_id`
//!   across chunks and duplicate seqs.
//!
//! Honest scope:
//! - Memory-bound assembler: holds all received chunks
//!   until the stream completes. Backpressure / streaming
//!   consumer isn't modeled.
//! - Caller picks `chunk_size`; must be ≤ MAX_PAYLOAD
//!   after serde framing overhead.
//!
//! Out of scope (follow-ups):
//! - Real wire-frame wrap/unwrap — the caller composes
//!   with ADR 0137 encode/decode.
//! - Backpressure / credit protocol.
//! - Cancellation / reset semantics.
//! - Interleaved streams on one connection.
//! - Compression per chunk.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::wire_frame::MAX_PAYLOAD;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    pub stream_id: u64,
    pub seq: u32,
    pub total: u32,
    pub body: Vec<u8>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChunkError {
    #[error("chunk_size must be between 1 and MAX_PAYLOAD ({MAX_PAYLOAD})")]
    InvalidChunkSize,
    #[error("payload too large to split with chunk_size={chunk_size}: total chunks {chunks} exceeds u32::MAX")]
    PayloadTooLarge { chunk_size: usize, chunks: u64 },
    #[error("chunk stream_id {got} mismatches assembler's {expected}")]
    StreamIdMismatch { expected: u64, got: u64 },
    #[error("chunk total {got} mismatches assembler's {expected}")]
    TotalMismatch { expected: u32, got: u32 },
    #[error("duplicate seq {seq} within stream {stream_id}")]
    DuplicateSeq { stream_id: u64, seq: u32 },
    #[error("seq {seq} >= total {total}")]
    SeqOutOfRange { seq: u32, total: u32 },
    #[error("assembler is not yet complete (received {received} of {total})")]
    Incomplete { received: u32, total: u32 },
}

pub type ChunkResult<T> = std::result::Result<T, ChunkError>;

/// ADR 0140 — split `payload` into N chunks tagged with
/// `stream_id`. Returns `Vec<Chunk>` in sequential order.
///
/// Empty payload produces a single chunk with empty body
/// (so the receiver always knows "this stream completed").
pub fn split(payload: &[u8], stream_id: u64, chunk_size: usize) -> ChunkResult<Vec<Chunk>> {
    if chunk_size == 0 || chunk_size > MAX_PAYLOAD {
        return Err(ChunkError::InvalidChunkSize);
    }
    if payload.is_empty() {
        return Ok(vec![Chunk {
            stream_id, seq: 0, total: 1, body: Vec::new(),
        }]);
    }
    let count = payload.len().div_ceil(chunk_size);
    if count as u64 > u32::MAX as u64 {
        return Err(ChunkError::PayloadTooLarge {
            chunk_size, chunks: count as u64,
        });
    }
    let total = count as u32;
    let mut out = Vec::with_capacity(count);
    for (i, slab) in payload.chunks(chunk_size).enumerate() {
        out.push(Chunk {
            stream_id, seq: i as u32, total,
            body: slab.to_vec(),
        });
    }
    Ok(out)
}

/// ADR 0140 — reassembler. Caller feeds each received
/// chunk; once `is_complete()`, `take()` yields the
/// concatenated payload in seq order.
#[derive(Debug, Default)]
pub struct Assembler {
    stream_id: Option<u64>,
    total: Option<u32>,
    received: BTreeMap<u32, Vec<u8>>,
}

impl Assembler {
    pub fn new() -> Self { Self::default() }

    pub fn push(&mut self, chunk: Chunk) -> ChunkResult<()> {
        if chunk.seq >= chunk.total {
            return Err(ChunkError::SeqOutOfRange {
                seq: chunk.seq, total: chunk.total,
            });
        }
        match self.stream_id {
            Some(sid) if sid != chunk.stream_id =>
                return Err(ChunkError::StreamIdMismatch {
                    expected: sid, got: chunk.stream_id,
                }),
            Some(_) => {}
            None => self.stream_id = Some(chunk.stream_id),
        }
        match self.total {
            Some(t) if t != chunk.total =>
                return Err(ChunkError::TotalMismatch {
                    expected: t, got: chunk.total,
                }),
            Some(_) => {}
            None => self.total = Some(chunk.total),
        }
        if self.received.contains_key(&chunk.seq) {
            return Err(ChunkError::DuplicateSeq {
                stream_id: chunk.stream_id, seq: chunk.seq,
            });
        }
        self.received.insert(chunk.seq, chunk.body);
        Ok(())
    }

    pub fn is_complete(&self) -> bool {
        match self.total {
            None => false,
            Some(t) => self.received.len() as u32 == t,
        }
    }

    pub fn received_count(&self) -> u32 {
        self.received.len() as u32
    }

    pub fn total(&self) -> Option<u32> { self.total }

    /// Consume the assembler and emit the concatenated
    /// payload. Returns `Incomplete` if any seq is missing.
    pub fn take(mut self) -> ChunkResult<Vec<u8>> {
        let total = self.total.ok_or(ChunkError::Incomplete {
            received: 0, total: 0,
        })?;
        if (self.received.len() as u32) != total {
            return Err(ChunkError::Incomplete {
                received: self.received.len() as u32, total,
            });
        }
        let mut out = Vec::new();
        for i in 0..total {
            let part = self.received.remove(&i).expect("complete-set");
            out.extend_from_slice(&part);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_small_payload_single_chunk() {
        let chunks = split(b"hello", 7, 100).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], Chunk {
            stream_id: 7, seq: 0, total: 1, body: b"hello".to_vec(),
        });
    }

    #[test]
    fn split_payload_at_chunk_boundary() {
        let chunks = split(b"abcdefghij", 1, 3).unwrap();
        assert_eq!(chunks.len(), 4);
        assert_eq!(chunks[0].body, b"abc");
        assert_eq!(chunks[1].body, b"def");
        assert_eq!(chunks[2].body, b"ghi");
        assert_eq!(chunks[3].body, b"j");
        for (i, c) in chunks.iter().enumerate() {
            assert_eq!(c.seq, i as u32);
            assert_eq!(c.total, 4);
        }
    }

    #[test]
    fn split_empty_payload_yields_marker_chunk() {
        let chunks = split(&[], 42, 100).unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].body.is_empty());
        assert_eq!(chunks[0].total, 1);
    }

    #[test]
    fn split_rejects_zero_chunk_size() {
        assert!(matches!(split(b"x", 1, 0),
            Err(ChunkError::InvalidChunkSize)));
    }

    #[test]
    fn split_rejects_oversize_chunk_size() {
        assert!(matches!(split(b"x", 1, MAX_PAYLOAD + 1),
            Err(ChunkError::InvalidChunkSize)));
    }

    #[test]
    fn round_trip_assembler() {
        let payload: Vec<u8> = (0..1000u32).map(|i| i as u8).collect();
        let chunks = split(&payload, 99, 128).unwrap();
        let mut a = Assembler::new();
        for c in chunks { a.push(c).unwrap(); }
        assert!(a.is_complete());
        assert_eq!(a.take().unwrap(), payload);
    }

    #[test]
    fn out_of_order_assembly_is_reordered() {
        let payload = b"hello, world";
        let chunks = split(payload, 1, 3).unwrap();
        let mut a = Assembler::new();
        // Reverse order.
        for c in chunks.into_iter().rev() { a.push(c).unwrap(); }
        assert_eq!(a.take().unwrap(), payload);
    }

    #[test]
    fn stream_id_mismatch_rejected() {
        let a_c = split(b"x", 1, 100).unwrap();
        let b_c = split(b"y", 2, 100).unwrap();
        let mut a = Assembler::new();
        a.push(a_c[0].clone()).unwrap();
        assert!(matches!(a.push(b_c[0].clone()),
            Err(ChunkError::StreamIdMismatch { .. })));
    }

    #[test]
    fn duplicate_seq_rejected() {
        let chunks = split(b"abcdef", 1, 3).unwrap();
        let mut a = Assembler::new();
        a.push(chunks[0].clone()).unwrap();
        let dup = Chunk { seq: 0, ..chunks[1].clone() };
        assert!(matches!(a.push(dup), Err(ChunkError::DuplicateSeq { .. })));
    }

    #[test]
    fn total_mismatch_rejected() {
        let chunks = split(b"abcdef", 1, 3).unwrap();
        let bad = Chunk { total: 99, ..chunks[1].clone() };
        let mut a = Assembler::new();
        a.push(chunks[0].clone()).unwrap();
        assert!(matches!(a.push(bad), Err(ChunkError::TotalMismatch { .. })));
    }

    #[test]
    fn seq_out_of_range_rejected() {
        let bad = Chunk { stream_id: 1, seq: 5, total: 2, body: vec![] };
        let mut a = Assembler::new();
        assert!(matches!(a.push(bad), Err(ChunkError::SeqOutOfRange { .. })));
    }

    #[test]
    fn incomplete_take_returns_error() {
        let chunks = split(b"abcdef", 1, 3).unwrap();
        let mut a = Assembler::new();
        a.push(chunks[0].clone()).unwrap();
        assert!(matches!(a.take(), Err(ChunkError::Incomplete { .. })));
    }

    #[test]
    fn chunk_serde_round_trip() {
        let c = Chunk {
            stream_id: 42, seq: 7, total: 10,
            body: vec![1, 2, 3, 4],
        };
        let json = serde_json::to_string(&c).unwrap();
        let back: Chunk = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn progress_accessors_report_correctly() {
        let chunks = split(b"abcdef", 1, 3).unwrap();
        let mut a = Assembler::new();
        assert_eq!(a.received_count(), 0);
        assert_eq!(a.total(), None);
        a.push(chunks[0].clone()).unwrap();
        assert_eq!(a.received_count(), 1);
        assert_eq!(a.total(), Some(2));
    }
}
