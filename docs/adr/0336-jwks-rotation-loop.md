# ADR 0336 — Scheduled JWKS rotation loop

## Status

Accepted. Closes the "scheduled rotation" follow-up from
ADR 0329. Completes the JWKS consumer shape: a long-lived
process can hold a shared `JwksCache` and never need to
manually call `fetch_from`.

## Context

ADR 0329 ships `JwksCache::fetch_from(client, url)` as a
one-shot async method. A production deployment needs the
cache refreshed on an interval so a rotated IdP key is
picked up without redeploying. The `/.well-known/jwks.json`
+ background-refresh pattern is the standard shape.

## Decision

Add two observability counters on `JwksCache`:

```rust
pub fn fetch_attempts(&self) -> u64;
pub fn fetch_errors  (&self) -> u64;
```

Every `fetch_from` call ticks `fetch_attempts`; failures
tick `fetch_errors` without touching the cached key set.

Add the rotation spawn:

```rust
pub fn spawn_rotation(
    &self, client: reqwest::Client, url: impl Into<String>,
    interval: Duration,
) -> tokio::task::JoinHandle<()>;
```

Semantics:

- Spawns a tokio task on the current runtime that loops
  `interval.tick() → fetch_from(client, &url)`.
- A transient fetch failure **does not kill the loop** —
  the next tick retries. Successes log at `debug`,
  failures at `warn` (via `tracing`).
- `MissedTickBehavior::Delay` so a slow process that
  falls behind doesn't burst-fire a chain of fetches.
- Returned `JoinHandle` can be awaited / aborted by the
  caller.

## Scope

**In scope:**
- Real tokio interval loop driving `fetch_from`.
- `tracing` logging at success / failure.
- `fetch_attempts` + `fetch_errors` observability.
- 3 new tests (13 total in `jwks`):
  1. `fetch_attempts` + `fetch_errors` counters tick
     correctly on a dead URL.
  2. Rotation task survives repeated fetch errors and
     keeps ticking (JoinHandle not finished after 200ms
     of error-only ticks).
  3. Cache stays stale + empty when every fetch fails,
     proving errors don't silently populate state.

**Out of scope** (dedicated follow-ups):
- ETag / Last-Modified conditional fetch so a 304 avoids
  the parse + replace path.
- Jittered interval to avoid thundering-herd on IdP
  during redeploy.
- Configurable retry back-off (currently flat
  `interval`).
- Shutdown signalling via a `watch::Receiver` /
  `tokio_util::sync::CancellationToken` — the caller
  currently aborts the `JoinHandle` directly.

## Consequences

- An operator wiring `spawn_rotation` into their service
  startup gets automatic JWKS freshness tracking with
  observable attempt / error counters.
- No new workspace deps — `tokio::time`, `tokio::task`,
  and `tracing` are already present.
- Verified behaviour: 200 ms of attempted fetches against
  an unused port tick at least 2 attempts, every one
  failed, task still alive — proven in the test
  `spawn_rotation_survives_fetch_errors_and_keeps_ticking`.
