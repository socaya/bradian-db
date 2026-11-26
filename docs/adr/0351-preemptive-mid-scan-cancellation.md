# ADR 0351 — Preemptive mid-scan cancellation at the wire layer

## Status

Accepted. Closes the explicit ADR 0346 follow-up:

> **Preemptive mid-statement cancellation** — ADR 0346
> only has cooperative between-statement cancellation.
> A SELECT that's in the middle of a scan can't be
> killed.

A `Cancel { session }` message (or `WireServer::cancel_session`
admin API call) now aborts an in-flight scan at the
next row-metered checkpoint, not only between
statements.

## Context

ADR 0346 shipped cooperative cancellation: a per-session
`Arc<AtomicBool>` flag is checked at the top of the
handler read loop. That handles the common case of a
client that wants to interrupt an idle session, but
leaves one gap — a client that fires a long-running
SELECT and then sends `Cancel` has to wait until the
SELECT fully materialises before the loop's next
cancel check runs.

The engine already has the plumbing needed:
`bradian_relational::runtime_limits::ScanControl` owns
a shared `Arc<AtomicBool>` and every scan path calls
`check()` at row and work-charge boundaries. The gap
was purely at the wire boundary — the handler built a
*fresh* `ScanControl` per call via
`SqlEngine::execute(sql)`, so its cancel flag was
disconnected from the handler's own.

## Decision

Two new constructors on `ScanControl`:

```rust
pub fn with_shared_cancel(flag: Arc<AtomicBool>) -> Self;
pub fn cancel_flag(&self) -> Arc<AtomicBool>;
```

`with_shared_cancel` builds a control with default
`ScanLimits` but uses the *caller-owned* `Arc` as its
cancel source. `cancel_flag` exposes the Arc so
additional sources (admin deadlines, parent cancel
contexts) can target the same bit.

The wire handler now, per Execute message:

1. Builds `ScanControl::with_shared_cancel(cancel_flag.clone())`
   where `cancel_flag` is the same per-session Arc
   that ADR 0346 installed in the
   `cancellations: HashMap<u64, Arc<AtomicBool>>`
   registry.
2. Calls `self.engine.execute_with_control(&sql, &control)`
   instead of `self.engine.execute(&sql)`.

Both cancellation entry points (the `Cancel { session }`
wire message and the admin `cancel_session` API) write
to that same Arc. The scan path observes it at the next
`check()`, returns `ResourceError::Cancelled`, and the
error propagates back as `SqlError`. The handler's
existing error branch writes
`WireMessage::ErrorMsg { code: 1, message: "…" }` for
the aborted statement; the next iteration's
top-of-loop check sees the flag true and writes
`ErrorMsg { code: 5 }` + `Bye`, same as the cooperative
path.

## Scope

**In scope:**
- `ScanControl::with_shared_cancel` + `cancel_flag()`
  on `bradian-relational`.
- Wire handler threads the per-session cancel Arc
  through `execute_with_control` per Execute.
- 3 new tests:
  1. `runtime_limits::tests::with_shared_cancel_observes_external_flips`
     — flipping the caller-owned Arc from outside
     fires `control.check()`.
  2. `runtime_limits::tests::cancel_flag_round_trips_the_same_arc`
     — `cancel_flag()` returns the identical Arc via
     `Arc::ptr_eq`.
  3. `wire::tests::execute_still_works_after_shared_cancel_plumbing`
     — regression guard that the happy path through
     `execute_with_control` returns rows + engine-
     sourced types identically to the pre-0351 shape.

**Out of scope** (dedicated follow-ups):
- **End-to-end mid-scan cancellation test.** Writing
  a deterministic "cancel exactly between row N and
  row N+1" test requires either a synthetic scan
  harness or a very large table seeded in advance.
  The unit test proves the plumbing; the integration
  test needs a dedicated harness.
- **Client-side deadline wiring.** A client can't
  yet pass a per-statement deadline into the wire
  protocol. When added, the server would OR that
  deadline into the same shared Arc via `cancel_flag()`.
- **Session-engine scan sharing.** The three session
  engines (`history_sql`, `domain_sql`, `mvcc_sql`)
  build their own `ScanControl` per session; wiring
  external cancel into those is a separate slice.
- **Cancellation granularity for non-SELECT paths.**
  INSERT / UPDATE / DELETE statements don't currently
  call `control.check()` at step boundaries. A DML
  that is already committing its WAL batch MUST NOT
  be cancelled mid-commit — the current behaviour is
  conservatively correct.

## Consequences

- A long-running SELECT scanning a large table now
  aborts promptly on cancel, instead of running to
  completion before the between-statement cancel
  check fires.
- The handler's per-Execute hot path builds one new
  `ScanControl`; that control's Arc is cloned from
  the per-session cancel flag — no extra allocations
  beyond the clone.
- Workspace is green under
  `cargo clippy --workspace --all-targets --locked
  -- -D warnings`. 71 wire tests pass.
- Pre-existing `base_expression_bundle::cast_bad_target_rejected`
  failure remains unrelated.
