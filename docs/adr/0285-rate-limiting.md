# ADR 0285 — Per-principal rate limiting on execute_as

## Status

Accepted.

## Context

`rate_limiter::TokenBucket` (ADR 0142) shipped as a
primitive. This slice wires it into `SqlEngine::execute_as`
so authenticated callers get per-principal admission control.

## Decision

- New `RateLimitPolicy { capacity, refill_per_second }` on
  `SqlEngine`.
- Fluent builder `SqlEngine::with_rate_limit(capacity,
  refill_per_second)` installs a policy. Default = unlimited.
- `rate_limit_buckets: Mutex<BTreeMap<String, TokenBucket>>`
  keyed by principal id. Lazily populated on first call.
- `charge_rate_limit(principal)` is invoked at the top of
  `execute_as`, BEFORE the authz check. Failed authz still
  charges — brute-force throttling is the point.
- Bucket exhaustion surfaces `SqlError::Resource(_)` with
  `resource: "rate_limit"`.

## Consequences

- Only `execute_as` is rate-limited. Direct `execute` calls
  (used for operator / setup paths) are not. Callers that
  want rate-limiting go through the authenticated path.
- Clock is read from `SystemTime::now()` each call. Clock
  skew / regression surfaces as `SqlError::Resource` too (the
  bucket's `advance` rejects regressions).
- Per-principal isolation: alice's throttling never affects
  bob's bucket.
- 4 cases in `tests/rate_limit.rs` cover: no policy → no
  limit; capacity=1 throttles 2nd request; per-principal
  isolation; authz-denied still charges.
