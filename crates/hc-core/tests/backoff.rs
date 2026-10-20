//! Reconnect backoff: exponential, jittered, capped. Asserted against a seeded `ChaCha8Rng`, so
//! the sequence is deterministic and the tests pin exact bounds.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use std::time::Duration;

use hc_core::backoff::Backoff;
use rand::SeedableRng as _;
use rand_chacha::ChaCha8Rng;

#[test]
fn backoff_is_exponential_with_jitter_and_capped() {
    let base = Duration::from_secs(1);
    let cap = Duration::from_secs(8);
    let factor = 2.0;

    // The un-jittered ceiling doubles each attempt, then saturates at the cap.
    let expected_ceilings = [
        Duration::from_secs(1),
        Duration::from_secs(2),
        Duration::from_secs(4),
        Duration::from_secs(8),
        Duration::from_secs(8), // capped
        Duration::from_secs(8), // capped
    ];

    let mut backoff = Backoff::new(base, cap, factor);
    let mut rng = ChaCha8Rng::seed_from_u64(0xC0FF_EE00);

    let mut delays = Vec::new();
    for (attempt, &ceiling) in expected_ceilings.iter().enumerate() {
        assert_eq!(
            backoff.ceiling(),
            ceiling,
            "ceiling at attempt {attempt} is exponential then capped"
        );
        let delay = backoff.next_delay(&mut rng);
        assert!(
            delay <= ceiling,
            "attempt {attempt}: jittered delay {delay:?} must not exceed its ceiling {ceiling:?}"
        );
        delays.push(delay);
    }

    // Full jitter is actually applied: the delays are not all pinned to the ceiling, and they are
    // not a constant sequence.
    assert!(
        delays.iter().zip(expected_ceilings).any(|(d, c)| *d < c),
        "at least one delay must be strictly below its ceiling — otherwise there is no jitter: \
         {delays:?}"
    );
    assert!(
        delays.windows(2).any(|w| w.first() != w.last()),
        "delays must vary across attempts, not be constant: {delays:?}"
    );

    // Deterministic under the same seed — this is what makes the assertion above reproducible.
    let mut backoff2 = Backoff::new(base, cap, factor);
    let mut rng2 = ChaCha8Rng::seed_from_u64(0xC0FF_EE00);
    let replayed: Vec<_> = (0..expected_ceilings.len())
        .map(|_| backoff2.next_delay(&mut rng2))
        .collect();
    assert_eq!(
        delays, replayed,
        "same seed must reproduce the same jittered sequence exactly"
    );

    // Reset returns to the base ceiling — a reconnected device starts its next outage from scratch.
    backoff.reset();
    assert_eq!(backoff.ceiling(), base, "reset returns to the base delay");
}

#[test]
fn default_backoff_never_panics_past_the_cap() {
    // The production default (base 1s, factor 1.2, `DEFAULT_CAP`). An offline appliance retries
    // forever and the counter resets only on a successful connect, so the ceiling must stay bounded
    // and never panic for any attempt, including `u32::MAX`. `1.2^attempt` overflows a `Duration`
    // around attempt 244, where `Duration::mul_f64` would panic before the cap is applied.
    let backoff = Backoff::default();
    // Read from the constant rather than restated.
    let cap = hc_core::backoff::DEFAULT_CAP;

    // Well below the overflow point: already saturated at the cap.
    assert_eq!(
        backoff.ceiling_at(100),
        cap,
        "a long-but-finite backoff is pinned at the cap"
    );

    // Past the overflow point, and the absolute worst case.
    for attempt in [244_u32, 1_000, 100_000, u32::MAX] {
        let ceiling = backoff.ceiling_at(attempt);
        assert_eq!(
            ceiling, cap,
            "attempt {attempt}: the ceiling must saturate at the cap, not overflow/panic"
        );
    }

    // The jittered delay is likewise bounded (and does not panic) at the extreme.
    let mut driven = Backoff::default();
    let mut rng = ChaCha8Rng::seed_from_u64(1);
    // Drive a few hundred real attempts through next_delay past the overflow point.
    for _ in 0..300 {
        let delay = driven.next_delay(&mut rng);
        assert!(
            delay <= cap,
            "every jittered delay stays within the cap: {delay:?}"
        );
    }
}

/// A sustained outage backs off much further than a blip.
///
/// An appliance that is refusing connections tracks its clients in a bounded table and should be
/// left alone; one that was off for ten seconds wants a fast retry. Full jitter halves the delays
/// in expectation, so the assertion pins the ceiling.
#[test]
fn a_long_outage_backs_off_far_beyond_a_minute() {
    let backoff = Backoff::default();

    // A blip is still answered quickly.
    assert!(
        backoff.ceiling_at(0) <= Duration::from_secs(2),
        "the first retry after a drop must still be prompt, got {:?}",
        backoff.ceiling_at(0)
    );

    // An outage that has gone on for dozens of attempts is not a blip.
    assert!(
        backoff.ceiling_at(60) >= Duration::from_mins(10),
        "a device that has refused us for dozens of attempts must be redialled in minutes, not \
         every minute forever, got {:?}",
        backoff.ceiling_at(60)
    );
}
