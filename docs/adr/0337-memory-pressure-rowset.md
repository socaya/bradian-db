# ADR 0337 — Memory-pressure gate on wire `RowSet` emission

## Status

Accepted. Closes the "memory-pressure integration" follow-
up from ADR 0327. Puts the admission harness to actual
work at the emission site where large result sets would
first hit real memory pressure.

## Context

ADR 0327 shipped `MemoryPressurePolicy` + harness as a
reusable primitive but didn't wire it into any
admission site. The wire handler is the natural first
consumer: a SELECT that returns millions of rows
materialises those rows in memory as the handler formats
them into a `WireMessage::RowSet` for emission. An
operator setting a per-connection memory budget needs
this to refuse before the materialised buffer blows out
the process RSS.

## Decision

Extend `WireServer`:

```rust
pub fn with_memory_pressure(
    mut self,
    harness: Arc<MemoryPressureHarness>,
) -> Self;
```

Pipeline per `Execute` that produces a `RowSet`:

1. Format the rows into the wire shape
   (`Vec<Vec<String>>`).
2. Compute `bytes = sum(column names) + sum(row cells)`.
3. If a harness is installed, `harness.admit(bytes)`:
   - Ok(guard) → emit the frame, drop the guard
     **after** the write so the in-flight bytes are
     released once the peer has received the data.
   - Err → reply `ErrorMsg { code: 4, message }` and
     keep the session alive.
4. If no harness is installed, emit as before
   (pre-0337 behaviour).

## Scope

**In scope:**
- Wire-handler admission against a shared
  `MemoryPressureHarness`.
- `ErrorMsg { code: 4 }` as the typed refusal; the
  message carries the harness's own `BudgetExceeded` /
  `InjectedFault` string so an operator sees the limit.
- RAII `MemoryPressureGuard` release on scope exit —
  the admitted bytes become free for the next `Execute`
  in the same session.
- 5 new tests (21 total in wire):
  1. Budget too small → `ErrorMsg(4)`, session alive,
     `stats.failures == 1`.
  2. Comfortable budget → `RowSet` emitted,
     `stats.failures == 0`, cumulative admitted > 0.
  3. Tight-exact budget → two successive SELECTs both
     succeed (proves release-on-drop between statements).
  4. No harness installed → baseline behaviour unchanged.
  5. `UnboundedMemory` harness never refuses.

**Out of scope** (dedicated follow-ups):
- Per-principal budget (right now the harness is
  server-wide). A per-session guard could key off the
  cert-derived Principal from ADR 0333.
- Streaming row emission that doesn't materialise the
  full result set in memory — the current wire format
  is one-shot `RowSet`. A chunked emission variant is a
  separate protocol change.
- Admission at the SQL executor level (before row
  materialisation). ADR 0327 explicitly said each
  admission site opts in incrementally; this slice
  closes the wire emission site.

## Consequences

- An operator wiring `.with_memory_pressure(h)` into
  their `WireServer` gets real per-RowSet admission with
  a visible refusal path.
- `bradian-relational::memory_pressure` is used by a
  real consumer; the ADR 0327 "pending integration"
  entry is closed.
- Baseline (no harness) behaviour is unchanged — the
  `without_memory_pressure_rowset_behaviour_is_unchanged`
  test is a regression guard.
