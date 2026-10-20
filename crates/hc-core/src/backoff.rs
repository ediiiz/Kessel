//! Reconnect backoff: exponential, jittered, capped.
//!
//! A ×1.2 growth factor capped at [`DEFAULT_CAP`]. Each delay is uniform in `[0, ceiling]` (full
//! jitter), so devices that went offline together do not reconnect in lockstep. The RNG is injected
//! so a seeded [`rand_chacha::ChaCha8Rng`] gives tests a reproducible sequence.

use std::time::Duration;

use rand::Rng;

/// The default growth factor.
pub const DEFAULT_FACTOR: f64 = 1.2;
/// The default ceiling, 15 minutes.
///
/// A failed connect looks the same whether the appliance is unreachable or rejecting the handshake,
/// and an appliance tracks its clients in a bounded table, so redialling a refusing device every
/// minute only adds load. Every successful connect resets the schedule, so a short outage is still
/// redialled within seconds; only a device that has failed dozens of times in a row is probed every
/// few minutes.
pub const DEFAULT_CAP: Duration = Duration::from_mins(15);
/// The default first-attempt base delay.
pub const DEFAULT_BASE: Duration = Duration::from_secs(1);

/// An exponential-with-jitter backoff schedule.
#[derive(Debug, Clone)]
pub struct Backoff {
    base: Duration,
    cap: Duration,
    factor: f64,
    attempt: u32,
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(DEFAULT_BASE, DEFAULT_CAP, DEFAULT_FACTOR)
    }
}

impl Backoff {
    /// A schedule with an explicit base, cap and factor.
    #[must_use]
    pub const fn new(base: Duration, cap: Duration, factor: f64) -> Self {
        Self {
            base,
            cap,
            factor,
            attempt: 0,
        }
    }

    /// The un-jittered ceiling for the current attempt: `min(cap, base * factor^attempt)`. The
    /// delay returned by [`Backoff::next_delay`] is uniform in `[0, ceiling]`.
    #[must_use]
    pub fn ceiling(&self) -> Duration {
        self.ceiling_at(self.attempt)
    }

    /// The un-jittered ceiling for a specific attempt number, independent of the internal counter.
    ///
    /// Never panics: `Duration::mul_f64` panics on a non-finite or too-large value, so growth is
    /// bounded before the multiply. For every `attempt` the result is in `[base.min(cap), cap]`.
    #[must_use]
    pub fn ceiling_at(&self, attempt: u32) -> Duration {
        let growth = self.factor.powi(exponent(attempt));
        // `factor^attempt` overflowed f64 (e.g. 1.2^244 == +inf): already past the cap.
        if !growth.is_finite() {
            return self.cap;
        }
        // f64 multiply saturates to +inf rather than panicking, so this comparison is safe even
        // when the product overflows; either way, at/over the cap the answer is the cap.
        let grown_secs = self.base.as_secs_f64() * growth;
        if grown_secs >= self.cap.as_secs_f64() {
            return self.cap;
        }
        // Here the grown delay is finite and strictly below the cap, so `mul_f64` cannot overflow.
        self.base.mul_f64(growth).min(self.cap)
    }

    /// The next delay: full jitter in `[0, ceiling]`, then advances the attempt counter.
    pub fn next_delay<R: Rng + ?Sized>(&mut self, rng: &mut R) -> Duration {
        let ceiling = self.ceiling();
        // Full jitter: a delay uniform in [0, ceiling]. `random_range` on the exclusive upper bound
        // never overshoots the ceiling, and a zero ceiling (base==cap==0 in tests) yields zero.
        let delay = if ceiling.is_zero() {
            Duration::ZERO
        } else {
            ceiling.mul_f64(rng.random::<f64>())
        };
        self.attempt = self.attempt.saturating_add(1);
        delay
    }

    /// Resets to the first attempt. Call on a successful connect, so the next outage starts from
    /// the base delay.
    pub const fn reset(&mut self) {
        self.attempt = 0;
    }

    /// The current attempt count (0 before the first [`Backoff::next_delay`]).
    #[must_use]
    pub const fn attempt(&self) -> u32 {
        self.attempt
    }
}

/// The attempt counter as an `i32` exponent for `powi`, saturating rather than wrapping — a device
/// offline for `> i32::MAX` reconnect attempts is already pinned at the cap.
fn exponent(attempt: u32) -> i32 {
    i32::try_from(attempt).unwrap_or(i32::MAX)
}
