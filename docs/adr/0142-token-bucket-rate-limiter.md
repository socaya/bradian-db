# ADR 0142 — Token-bucket rate limiter primitive (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-17.** Companion to ADR 0141
(query budget) and ADR 0114 (lock timeout). Provides a
bounded-admission primitive for the future
`bradian-serving` request path.

## 1. Scope

New module `bradian_relational::rate_limiter`:

- `TokenBucket { capacity, refill_per_second, tokens,
  last_micros }` — classic token bucket.
- `new(capacity, refill_per_second)` — starts full.
- `advance(now_micros)` — idempotent refill from
  `last_micros` to `now_micros`; clamps at `capacity`.
- `try_consume(n, now_micros)` — refill then try to
  spend; returns `Ok(())` and decrements iff
  sufficient, else `Err(Insufficient)` without side
  effects.
- `RateLimitError { OverCapacity / Insufficient /
  ClockRegression }`.

Deterministic: all time arrives via injected
`now_micros`. Zero refill is a static budget.

## 2. Out of scope (follow-ups)

- **Thread-safe wrapper** — primitive is `Send`.
  Callers add `Mutex<TokenBucket>`.
- **Fair-share / weighted-fair-queueing** — one
  bucket per caller; priorities are a follow-up.
- **Burst allowance distinct from capacity** — not
  modeled.
- **Wall-clock-backed tick** — caller injects
  micros from any monotone clock.
- **Backpressure signaling** — on refusal the caller
  decides between retry / HTTP 429 / sleep.

## 3. Compliance

- New bucket starts full.
- Consume decrements tokens.
- Over-capacity request errors.
- Insufficient request errors without side effects.
- Refill scales with elapsed time; clamps at capacity.
- Zero refill is a static budget.
- Clock regression rejected.
- Advance is idempotent (no double-refill on same
  timestamp).
- Interleaved consume+refill composes correctly.
- 11 new tests in `rate_limiter::tests`.

## 4. Honest status

Foundation only. **BRD-ADMISSION-002 request rate
limiting** is not yet satisfied because no transport
or engine consults the bucket. A future
`bradian-serving` ADR wraps it in a `tokio` middleware.
