# ADR 0346 — Cooperative wire-session cancellation

## Status

Accepted. Closes the "wire cancellation" follow-up from
the typed-rows ADR series. Ships a real cancel primitive
in the wire protocol along with a server-side admin API.

## Context

Prior ADRs gave operators every signal they need about a
running session EXCEPT the ability to say "stop that
session." A long-running `SELECT` or a misbehaving client
had no out-of-band way to be terminated — the server
couldn't close a specific session without closing the
whole listener.

## Decision

Two interlocked primitives, backed by one per-session
`AtomicBool` flag:

1. **Server API** —
   `WireServer::cancel_session(session_id: u64) -> bool`.
   Flips the flag in the registry. Returns `true` if the
   session was live; `false` if it has already closed.
   `is_session_cancelled(id)` is the read-side probe.
2. **Wire message** — `WireMessage::Cancel { session }`.
   A client (or an admin channel) sends it; the server
   acks with `Command { affected: 0 }` and routes the
   flag flip through the same registry. A self-targeted
   `Cancel` flips the current session's local flag; a
   cross-session `Cancel` goes through `cancel_session`.

The handler's main loop checks the flag at the top of
every iteration. When set, it emits:

```
ErrorMsg { code: 5, message: "session {N} cancelled" }
Bye
```

and returns. An `AuditEntry` with `action: "session_cancel"`
is written if an audit log is installed.

Registration lifecycle: a `CancelGuard` inserts the flag
on session open and removes it on drop, so a closed
session doesn't leak its slot.

## Scope

**In scope:**
- Cooperative cancellation: the flag is checked between
  wire frames, not during engine execution. An in-flight
  `SqlEngine::execute` completes before the cancel
  message fires.
- Admin API (`cancel_session`) + wire message (`Cancel`)
  sharing one flag.
- Session registry lifecycle via `CancelGuard`.
- 3 new tests on top of the 39 pre-existing (42 wire
  tests total):
  1. `cancel_session` on a live session causes the
     client to observe `ErrorMsg { code: 5 }` then `Bye`;
     the session is removed from the registry afterward.
  2. Self-targeted `Cancel` frame → ack `Command(0)` →
     `ErrorMsg(5)` → `Bye` on the same stream.
  3. `cancel_session(unknown_id)` returns `false`.

**Out of scope** (dedicated follow-ups):
- **Truly preemptive** mid-statement cancellation. The
  engine's `execute` is synchronous and holds no handle
  to the cancellation flag. Threading it in is a
  separate engine-side ADR; the cooperative primitive
  here lets an operator abort an idle-but-open session
  today.
- Timeout-based auto-cancel (`cancel_session` after N
  seconds of inactivity).
- A `CancellableToken` abstraction over
  `cancel_session` for background services.

## Consequences

- An operator can terminate an idle or politely-paused
  session without closing the listener.
- A client can self-cancel after a known checkpoint
  (useful for idempotent abort of a batch mid-way).
- 42 wire tests total; workspace stays green under
  `cargo clippy --workspace --all-targets --locked
  -- -D warnings`.
