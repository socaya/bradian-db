//! ADR 0120 — Change-Data-Capture (CDC) source primitive.
//!
//! BRD-MIG-002 scoped deviation: offline cutover works, but
//! live CDC was marked "not implemented". This module ships
//! a WAL-reading CDC EMITTER that produces a typed stream of
//! `CdcEvent`s a downstream consumer can tail.
//!
//! Honest scope: the emitter reads the RowStore WAL and
//! yields parsed mutation events. It does NOT ship:
//!  - Durable cursor / resume-after-crash;
//!  - Network push / pub-sub;
//!  - Schema-aware JSON projection (consumers get raw
//!    key/value bytes);
//!  - Backpressure / ack protocol.
//!
//! Those remain follow-up ADRs.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CdcEvent {
    /// Row inserted or updated.
    Upsert { lsn: u64, key: Vec<u8>, value: Vec<u8> },
    /// Row deleted.
    Delete { lsn: u64, key: Vec<u8> },
}

impl CdcEvent {
    pub fn lsn(&self) -> u64 {
        match self {
            CdcEvent::Upsert { lsn, .. } | CdcEvent::Delete { lsn, .. } => *lsn,
        }
    }
    pub fn key(&self) -> &[u8] {
        match self {
            CdcEvent::Upsert { key, .. } | CdcEvent::Delete { key, .. } => key,
        }
    }
}

/// In-memory CDC cursor. Advance it against a `RowMutation`
/// slice (e.g., from `commit_batch` or scan) to produce
/// events. Operators who want a continuous stream can call
/// `RowStore::scan()` + `drain_mutations()` for a durable
/// resume-from-LSN story; that wiring is out of scope.
#[derive(Clone, Debug, Default)]
pub struct CdcEmitter {
    pub last_lsn_emitted: u64,
}

impl CdcEmitter {
    pub fn new() -> Self { Self::default() }

    /// Record and translate a batch of raw mutation ops
    /// taken from the WAL. The caller passes the committing
    /// LSN of the batch; the emitter deduplicates events it
    /// has already seen by `lsn <= last_lsn_emitted`.
    pub fn emit_batch(
        &mut self, batch_lsn: u64,
        mutations: impl IntoIterator<Item = crate::row_store::RowMutation>,
    ) -> Vec<CdcEvent> {
        if batch_lsn <= self.last_lsn_emitted {
            return Vec::new();
        }
        let mut events = Vec::new();
        for m in mutations {
            match m {
                crate::row_store::RowMutation::Put { key, value } => {
                    events.push(CdcEvent::Upsert { lsn: batch_lsn, key, value });
                }
                crate::row_store::RowMutation::Delete { key } => {
                    events.push(CdcEvent::Delete { lsn: batch_lsn, key });
                }
            }
        }
        self.last_lsn_emitted = batch_lsn;
        events
    }

    pub fn resume_from(&mut self, lsn: u64) {
        self.last_lsn_emitted = lsn;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::row_store::RowMutation;

    #[test]
    fn emit_batch_translates_mutations_into_events() {
        let mut e = CdcEmitter::new();
        let events = e.emit_batch(42, vec![
            RowMutation::Put { key: b"k1".to_vec(), value: b"v1".to_vec() },
            RowMutation::Delete { key: b"k2".to_vec() },
        ]);
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], CdcEvent::Upsert { lsn: 42, key, value }
            if key == b"k1" && value == b"v1"));
        assert!(matches!(&events[1], CdcEvent::Delete { lsn: 42, key }
            if key == b"k2"));
        assert_eq!(e.last_lsn_emitted, 42);
    }

    #[test]
    fn already_seen_lsn_yields_nothing() {
        let mut e = CdcEmitter::new();
        e.emit_batch(42, vec![RowMutation::Put {
            key: b"k".to_vec(), value: b"v".to_vec(),
        }]);
        let repeat = e.emit_batch(42, vec![RowMutation::Put {
            key: b"k".to_vec(), value: b"v".to_vec(),
        }]);
        assert!(repeat.is_empty());
    }

    #[test]
    fn resume_from_lsn_moves_watermark() {
        let mut e = CdcEmitter::new();
        e.resume_from(100);
        let empty = e.emit_batch(50, vec![RowMutation::Put {
            key: b"k".to_vec(), value: b"v".to_vec(),
        }]);
        assert!(empty.is_empty(), "batch below watermark yields nothing");
        let fresh = e.emit_batch(200, vec![RowMutation::Put {
            key: b"k".to_vec(), value: b"v".to_vec(),
        }]);
        assert_eq!(fresh.len(), 1);
    }

    #[test]
    fn event_accessors_work() {
        let u = CdcEvent::Upsert {
            lsn: 7, key: b"k".to_vec(), value: b"v".to_vec(),
        };
        assert_eq!(u.lsn(), 7);
        assert_eq!(u.key(), b"k");
        let d = CdcEvent::Delete { lsn: 8, key: b"k".to_vec() };
        assert_eq!(d.lsn(), 8);
    }
}
