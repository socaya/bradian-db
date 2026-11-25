# ADR 0327 — Memory-pressure fault harness

## Status

Accepted. Closes the G1-MEMORY qualification item (the
memory-pressure fault harness called out as the one
remaining code-axis gap in section 3.4 of the platform
gaps plan). ADR 0320 and 0321 slots are already used by
earlier `HistorySqlEngine` memory sampling and POSIX-ACL
work; this ADR takes the next free number past the
infrastructure-deferred range.

## Context

`runtime_limits::ScanLimits::max_bytes` already caps the
decoded working-set bytes of a single query. What the
Gate 1 qualification register actually wants is an
**injectable dynamic pressure source** that simulates
low-memory conditions regardless of the static scan cap —
so a store (or an engine test harness) can be asked
"what would happen if your next 64 KiB allocation
failed?" without rebuilding the binary.

## Decision

New module `bradian_relational::memory_pressure` exposing:

```rust
pub trait MemoryPressurePolicy: Send + Sync {
    /// Admit `bytes` into the budget. Returns Err if the
    /// policy refuses.
    fn admit(&self, bytes: usize)   -> Result<(), MemoryPressureError>;
    /// Release previously-admitted bytes.
    fn release(&self, bytes: usize);
    /// Cumulative admitted bytes (across all currently
    /// outstanding admits).
    fn in_flight(&self) -> usize;
}

pub struct UnboundedMemory;        // never fails — default
pub struct FixedBudget { limit }   // refuses past a cap
pub struct FailAfterBytes { trip } // one-shot: admit until cumulative >= trip, then refuse once

pub struct MemoryPressureHarness {
    policy:   Box<dyn MemoryPressurePolicy>,
    admitted: AtomicU64,
    peak:     AtomicU64,
    failures: AtomicU64,
}
impl MemoryPressureHarness {
    pub fn admit(&self, bytes: usize)   -> Result<MemoryPressureGuard<'_>, MemoryPressureError>;
    pub fn stats(&self) -> MemoryPressureStats;
}
```

Semantics:

- `FixedBudget` is the realistic workload-cap policy: an
  `admit(n)` moves `in_flight += n` and refuses if the
  new total would exceed `limit`. `release(n)` subtracts.
- `FailAfterBytes` is the injectable fault: the Nth
  admit that would push the cumulative past `trip` is
  refused (once); subsequent admits succeed again. Useful
  for scripted "what if this allocation failed?"
  scenarios in the existing fault-sweep harness
  (ADR 0254, 0255, 0286, 0289).
- `MemoryPressureHarness` wraps any policy with
  observability: current in-flight, peak-ever admitted,
  cumulative failures. Returning a `MemoryPressureGuard`
  from `admit` lets the caller `drop` the guard to
  auto-release — the RAII shape every bounded primitive
  in the crate already uses.

## Scope

**In scope:**
- Three real policy impls (`UnboundedMemory`,
  `FixedBudget`, `FailAfterBytes`) with real admission
  bookkeeping.
- Observability wrapper with cumulative + peak +
  failures counters.
- RAII guard that `release`s on drop.
- 8 unit tests: unbounded never fails, fixed budget
  refuses past cap + accepts after release, fail-after
  trips once and then recovers, guard releases on drop,
  concurrent admitters share the budget correctly,
  harness stats update on admit / fail, zero-byte admit
  is a no-op, release of more than admitted is
  saturating (never underflows).

**Out of scope** (dedicated follow-ups):
- Integration with every store — this slice ships the
  reusable primitive. Each engine that opts in (SQL
  executor's row accumulation, index builder, backup
  staging) does so as a bounded follow-up.
- Automatic hook into the existing `ScanLimits` — the
  caller chooses which admission site uses which policy.

## Consequences

- An operator or test harness can wire a
  `FixedBudget(16 MiB)` into any admission site that
  takes `&dyn MemoryPressurePolicy` and observe real
  refusal semantics.
- A fault-injection test can wire a `FailAfterBytes` to
  simulate a specific allocation failure and then
  recover.
- Closes G1-MEMORY on the code axis. Hardware-level
  RSS qualification remains infrastructure.
