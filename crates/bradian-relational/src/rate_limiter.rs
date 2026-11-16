//! ADR 0142 — token-bucket rate limiter primitive.
//!
//! BRD-ADMISSION-002 "request rate limiting" is OPEN. The
//! future network server (`bradian-serving`) will need a
//! bounded-admission primitive for incoming requests.
//! This module ships the classic token bucket as a pure
//! state machine the caller advances.
//!
//! Honest scope:
//! - `TokenBucket { capacity, refill_per_second }`.
//! - `try_consume(n, now_micros)` — monotone clock
//!   injection (so tests are deterministic).
//! - Overflow clamped to `capacity`; backlog is NOT
//!   credited across infinite idle time.
//! - Zero refill supported (static budget).
//!
//! Out of scope (follow-ups):
//! - Thread-safe wrapper — the primitive is `Send`;
//!   callers add a `Mutex<TokenBucket>` or
//!   `parking_lot` guard.
//! - Fair-share / weighted-fair-queueing — one bucket
//!   per caller; priorities are a follow-up.
//! - Burst allowance distinct from capacity.
//! - Wall-clock backed tick — caller injects micros.
//! - Backpressure signaling — on refusal the caller
//!   decides between retry / 429.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RateLimitError {
    #[error("requested {requested} tokens exceeds bucket capacity {capacity}")]
    OverCapacity { requested: u64, capacity: u64 },
    #[error("not enough tokens: requested {requested}, available {available}")]
    Insufficient { requested: u64, available: u64 },
    #[error("clock went backwards from {last_micros} to {new_micros}")]
    ClockRegression { last_micros: u64, new_micros: u64 },
}

pub type RateLimitResult<T> = std::result::Result<T, RateLimitError>;

#[derive(Debug)]
pub struct TokenBucket {
    capacity: u64,
    refill_per_second: u64,
    tokens: u64,
    last_micros: u64,
}

impl TokenBucket {
    /// `capacity` tokens; `refill_per_second` added at
    /// line rate. Starts full.
    pub fn new(capacity: u64, refill_per_second: u64) -> Self {
        Self {
            capacity, refill_per_second,
            tokens: capacity, last_micros: 0,
        }
    }

    /// Current tokens (after any pending refill).
    pub fn tokens(&self) -> u64 { self.tokens }
    pub fn capacity(&self) -> u64 { self.capacity }

    /// Advance the clock. Idempotent: refills are
    /// computed from `last_micros` to `now_micros`.
    pub fn advance(&mut self, now_micros: u64) -> RateLimitResult<()> {
        if now_micros < self.last_micros {
            return Err(RateLimitError::ClockRegression {
                last_micros: self.last_micros,
                new_micros: now_micros,
            });
        }
        if self.refill_per_second == 0 {
            self.last_micros = now_micros;
            return Ok(());
        }
        let elapsed_micros = now_micros - self.last_micros;
        // refill = elapsed_micros * rate / 1_000_000
        let add = (elapsed_micros as u128)
            .saturating_mul(self.refill_per_second as u128)
            / 1_000_000_u128;
        self.tokens = self.tokens
            .saturating_add(add.min(u64::MAX as u128) as u64)
            .min(self.capacity);
        self.last_micros = now_micros;
        Ok(())
    }

    /// ADR 0142 — try to consume `n` tokens. Returns
    /// `Ok(())` and decrements iff sufficient; else
    /// `Err(Insufficient)` without side effects.
    pub fn try_consume(&mut self, n: u64, now_micros: u64) -> RateLimitResult<()> {
        if n > self.capacity {
            return Err(RateLimitError::OverCapacity {
                requested: n, capacity: self.capacity,
            });
        }
        self.advance(now_micros)?;
        if self.tokens < n {
            return Err(RateLimitError::Insufficient {
                requested: n, available: self.tokens,
            });
        }
        self.tokens -= n;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_bucket_starts_full() {
        let b = TokenBucket::new(100, 10);
        assert_eq!(b.tokens(), 100);
        assert_eq!(b.capacity(), 100);
    }

    #[test]
    fn consume_decrements_tokens() {
        let mut b = TokenBucket::new(100, 0);
        b.try_consume(30, 0).unwrap();
        assert_eq!(b.tokens(), 70);
    }

    #[test]
    fn consume_over_capacity_errors() {
        let mut b = TokenBucket::new(100, 0);
        assert!(matches!(b.try_consume(101, 0),
            Err(RateLimitError::OverCapacity { .. })));
    }

    #[test]
    fn consume_insufficient_errors_without_side_effect() {
        let mut b = TokenBucket::new(10, 0);
        b.try_consume(8, 0).unwrap();  // 2 left
        assert!(matches!(b.try_consume(5, 0),
            Err(RateLimitError::Insufficient { requested: 5, available: 2 })));
        assert_eq!(b.tokens(), 2);  // unchanged
    }

    #[test]
    fn refill_scales_with_elapsed_time() {
        let mut b = TokenBucket::new(100, 1_000);  // 1k/s
        b.try_consume(50, 0).unwrap();
        // Advance 100 ms → +100 tokens; cap at 100.
        b.advance(100_000).unwrap();
        assert_eq!(b.tokens(), 100);
    }

    #[test]
    fn refill_partial_advance() {
        let mut b = TokenBucket::new(100, 1_000);  // 1k/s
        b.try_consume(80, 0).unwrap();  // 20 left
        b.advance(50_000).unwrap();  // 50 ms → +50
        assert_eq!(b.tokens(), 70);
    }

    #[test]
    fn zero_refill_is_static_budget() {
        let mut b = TokenBucket::new(10, 0);
        b.try_consume(5, 0).unwrap();
        b.advance(10_000_000).unwrap();  // 10 s elapsed
        assert_eq!(b.tokens(), 5);  // no refill
    }

    #[test]
    fn capacity_clamps_refill() {
        let mut b = TokenBucket::new(10, 1_000_000);
        b.try_consume(5, 0).unwrap();
        b.advance(1_000_000).unwrap();  // 1 s elapsed; +1M
        assert_eq!(b.tokens(), 10);  // clamped at capacity
    }

    #[test]
    fn clock_regression_rejected() {
        let mut b = TokenBucket::new(10, 100);
        b.try_consume(1, 1_000).unwrap();
        assert!(matches!(b.try_consume(1, 500),
            Err(RateLimitError::ClockRegression { .. })));
    }

    #[test]
    fn advance_is_idempotent() {
        let mut b = TokenBucket::new(100, 1_000);
        b.try_consume(50, 0).unwrap();  // 50 left
        b.advance(50_000).unwrap();      // +50 → 100
        b.advance(50_000).unwrap();      // no change
        assert_eq!(b.tokens(), 100);
    }

    #[test]
    fn interleaved_consume_refill() {
        let mut b = TokenBucket::new(10, 10_000);  // 10k/s = 1 token/100µs
        b.try_consume(10, 0).unwrap();  // 0 left
        assert!(matches!(b.try_consume(1, 50),
            Err(RateLimitError::Insufficient { .. })));
        // After the refused attempt, last_micros moved to 50 and
        // the accrued 0 tokens were booked. From 50 → 300 is 250 µs
        // → floor(250 * 10000 / 1_000_000) = 2 tokens; consume 1 → 1
        // left.
        b.try_consume(1, 300).unwrap();
        assert_eq!(b.tokens(), 1);
    }
}
