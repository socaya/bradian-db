# ADR 0114 — Operation-lock timeout policy (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch C C1 — partial close
of BRD-TXN-003 lock-timeout item.

## Scope

- `bradian_relational::lock_policy::LockPolicy{Blocking,
  Timeout(Duration)}` wraps `parking_lot::Mutex::try_lock_for`.
- `acquire(&mutex, policy) -> Result<Guard, LockError>`.
- Backward-compatible: `Blocking` matches pre-ADR-0114 engine
  behaviour (the existing `operation.lock()` call sites are
  unchanged).

## Out of scope

- **Engine opt-in wiring** — engines continue to call
  `operation.lock()` directly. Switching their call sites to
  this policy wrapper is a mechanical follow-up.
- **Deadlock detection** (C2) — a separate module.
- **Multi-process / cross-node lock timeouts** — §13 blocked.

## Compliance

1. `Blocking` returns the guard like `Mutex::lock()`.
2. `Timeout(50ms)` on a held mutex returns `LockError::Timeout
   { ms: 50 }` after ~50ms.
3. `Timeout` on an unheld mutex returns the guard.
4. 3 unit tests in `src/lock_policy.rs`.
