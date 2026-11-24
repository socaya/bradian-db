# ADR 0316 — Relational wire protocol + reference driver: skeleton

## Status

Accepted as a skeleton. Ninth slice of P2.

## Context

A real Bradian wire protocol needs a stable binary frame
format, authentication handshake, statement + resultset
messages, cancellation, and a reference driver each
language SDK can mirror. Gate-sized.

## Decision

New module `bradian_serving::wire` with typed messages and
an in-process **reference driver** that exchanges them
without a real socket:

```rust
pub enum WireMessage {
    Hello    { version: u32, principal: String },
    HelloOk  { session: u64 },
    Execute  { sql: String },
    RowSet   { columns: Vec<String>, rows: Vec<Vec<String>> },
    Command  { affected: u64 },
    ErrorMsg { code: u32, message: String },
    Bye,
}
pub struct ReferenceDriver { ... }
impl ReferenceDriver {
    pub fn connect(principal: impl Into<String>) -> DriverResult<Self>;
    pub fn execute(&mut self, sql: &str)          -> DriverResult<WireMessage>;
    pub fn close  (self);
}
```

Reference driver sanity: send `Hello`, server replies
`HelloOk`; `execute("SELECT …")` echoes a canned row set;
`close` sends `Bye`.

Honest scope: pure in-process message pass. No TCP, no
TLS binding (ADR 0283 already ships that layer), no SQL
execution wired. Follow-ups bolt the serving layer into
the message handler.

## Consequences

- 2 unit tests: connect handshake round-trips; execute
  returns a row set shape.
