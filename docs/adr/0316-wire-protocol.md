# ADR 0316 — Relational wire protocol + reference driver

## Status

Accepted. Replaces the 2026-11-24 skeleton with a **real
tokio-based TCP server + client** bound to a live
`SqlEngine`.

## Context

The skeleton exchanged `WireMessage`s in-process with a
canned reply logic. A real wire protocol needs to go over
a real socket, frame its messages so a peer can't desync
the stream, and dispatch SQL to a live engine.

## Decision

Module `bradian_serving::wire`:

```rust
pub const MAX_FRAME_BYTES: u32 = 1024 * 1024;
pub enum WireMessage { Hello / HelloOk / Execute / RowSet / Command / ErrorMsg / Bye }
pub struct WireServer { Arc<SqlEngine>, next_session }
pub struct WireClient { TcpStream, session }
```

Framing: `[4-byte big-endian length][JSON body]`. Any
length > `MAX_FRAME_BYTES` is refused on read, so a buggy
or hostile peer cannot allocate unbounded memory on the
far side.

`WireServer::serve_one(listener)`:
- Accepts one TCP connection.
- Requires the first frame to be `Hello`; anything else
  errors out with `Handshake(...)`.
- Replies with `HelloOk { session = N }` where N is a
  monotonic server-side counter.
- Loops reading `Execute` / `Bye` frames. Each `Execute`
  is dispatched through `SqlEngine::execute`:
  `Rows { columns, rows }` → `RowSet`;
  `Command { affected }` → `Command`;
  error → `ErrorMsg { code, message }`.
- `Bye` acknowledges with `Bye` and closes.
- A client FIN mid-session is treated as orderly close.

`WireClient::connect(addr, principal)`:
- Opens a `TcpStream`, sends `Hello { version: 1, principal }`.
- Reads `HelloOk` or errors.
- `execute(sql)` sends one `Execute`, awaits one reply.
- `bye()` sends `Bye`, reads the ack, returns.

## Scope

**In scope:**
- Real tokio `TcpListener` + `TcpStream`.
- Length-prefixed JSON framing with `MAX_FRAME_BYTES` cap.
- Handshake rejection on non-Hello first frame.
- SQL errors surfaced as `ErrorMsg` without tearing down
  the connection.
- 5 integration-style unit tests (all `#[tokio::test]`
  on real sockets at 127.0.0.1:0):
  - round-trip SELECT through the engine,
  - SQL error → ErrorMsg,
  - multiple Executes in one session,
  - oversized frame rejected,
  - handshake rejects non-Hello first frame.

**Out of scope** (dedicated follow-ups):
- TLS wrapping (ADR 0283 ships rustls behind the HTTP
  stack; wrapping `TcpStream` in `TlsStream` is a
  dedicated slice).
- Authentication of the `principal` field against
  `UserCatalog` (ADR 0247/0250 plumbing is present in
  `auth.rs`).
- Prepared statements, cursor-based streaming of large
  row sets, cancellation, server-push notifications.
- Row-value typing — row cells are currently emitted as
  debug-printed strings so the framing is complete; a
  typed wire encoding is the natural next slice.

## Consequences

- An external process can speak the Bradian wire
  protocol over TCP and get back real SQL results.
- `MAX_FRAME_BYTES` bounds per-frame memory on either
  end; a malicious peer cannot force unbounded allocation.
- The 2026-11-24 "SKELETON" status on 0316 is retired.
