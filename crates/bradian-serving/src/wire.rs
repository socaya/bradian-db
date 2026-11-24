//! ADR 0316 — Relational wire protocol + reference driver
//! **skeleton**.
//!
//! Pure in-process message pass. No TCP, no TLS binding
//! (ADR 0283 already ships that layer), no SQL execution
//! wired. Follow-ups bolt the serving layer into the
//! message handler.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireMessage {
    Hello    { version: u32, principal: String },
    HelloOk  { session: u64 },
    Execute  { sql: String },
    RowSet   { columns: Vec<String>, rows: Vec<Vec<String>> },
    Command  { affected: u64 },
    ErrorMsg { code: u32, message: String },
    Bye,
}

#[derive(Debug, Error)]
pub enum DriverError {
    #[error("wire handshake mismatch: {0}")]
    Handshake(String),
    #[error("driver already closed")]
    Closed,
}
pub type DriverResult<T> = Result<T, DriverError>;

#[derive(Debug)]
pub struct ReferenceDriver {
    session: u64,
    closed:  bool,
}

impl ReferenceDriver {
    pub fn connect(_principal: impl Into<String>) -> DriverResult<Self> {
        // In-process handshake: Hello -> HelloOk.
        let hello = WireMessage::Hello { version: 1, principal: "stub".into() };
        let reply = Self::reply_to(&hello);
        match reply {
            WireMessage::HelloOk { session } => Ok(Self { session, closed: false }),
            _ => Err(DriverError::Handshake("expected HelloOk".into())),
        }
    }

    pub fn session(&self) -> u64 { self.session }

    pub fn execute(&mut self, sql: &str) -> DriverResult<WireMessage> {
        if self.closed { return Err(DriverError::Closed); }
        Ok(Self::reply_to(&WireMessage::Execute { sql: sql.into() }))
    }

    pub fn close(mut self) {
        self.closed = true;
        let _ = Self::reply_to(&WireMessage::Bye);
    }

    /// Server-side reply logic — deliberately canned.
    fn reply_to(msg: &WireMessage) -> WireMessage {
        match msg {
            WireMessage::Hello   { .. } => WireMessage::HelloOk { session: 1 },
            WireMessage::Execute { sql } => {
                // Echo the SQL back as a one-row, one-column row set.
                WireMessage::RowSet {
                    columns: vec!["sql".into()],
                    rows:    vec![vec![sql.clone()]],
                }
            }
            WireMessage::Bye => WireMessage::Bye,
            other => WireMessage::ErrorMsg {
                code: 1, message: format!("unexpected message: {other:?}") },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_handshake_round_trips() {
        let d = ReferenceDriver::connect("alice").unwrap();
        assert_eq!(d.session(), 1);
    }

    #[test]
    fn execute_returns_a_row_set_echoing_the_statement() {
        let mut d = ReferenceDriver::connect("bob").unwrap();
        let rs = d.execute("SELECT 1").unwrap();
        match rs {
            WireMessage::RowSet { columns, rows } => {
                assert_eq!(columns, vec!["sql".to_string()]);
                assert_eq!(rows,   vec![vec!["SELECT 1".to_string()]]);
            }
            other => panic!("expected RowSet, got {other:?}"),
        }
        d.close();
    }
}
