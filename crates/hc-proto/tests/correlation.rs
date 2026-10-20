//! Request/response correlation by msgID, including out-of-order delivery and timeout.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use std::cell::Cell;

use hc_proto::{Clock, Correlator};

/// A fake, hand-advanced clock — the crate opens no real timer.
struct TestClock(Cell<u64>);

impl TestClock {
    const fn at(ms: u64) -> Self {
        Self(Cell::new(ms))
    }
    fn set(&self, ms: u64) {
        self.0.set(ms);
    }
}

impl Clock for TestClock {
    fn now_ms(&self) -> u64 {
        self.0.get()
    }
}

/// A later request's response arrives first, so a FIFO matcher would mis-pair it.
#[test]
fn correlation_resolves_by_msgid() {
    let mut correlator = Correlator::new();
    // Five GETs outstanding at once, as in the captured handshake.
    correlator.register(774, "/ci/authentication", 0);
    correlator.register(775, "/ci/info", 0);
    correlator.register(777, "/ni/info", 0);
    correlator.register(778, "/ro/allMandatoryValues", 0);
    correlator.register(779, "/ro/allDescriptionChanges", 0);
    assert_eq!(correlator.len(), 5);

    // Deliver out of arrival order: 778 first. A FIFO matcher would return 774's resource.
    assert_eq!(
        correlator.resolve(778).as_deref(),
        Some("/ro/allMandatoryValues"),
        "must pair by msgID, not by arrival position"
    );
    assert_eq!(
        correlator.resolve(774).as_deref(),
        Some("/ci/authentication")
    );
    assert_eq!(
        correlator.resolve(779).as_deref(),
        Some("/ro/allDescriptionChanges")
    );

    // An unknown msgID is uncorrelated, not fatal (routes to the general handler).
    assert!(correlator.resolve(999).is_none());
    // Resolved ids are gone; two remain.
    assert!(!correlator.is_pending(778));
    assert_eq!(correlator.len(), 2);
}

/// A pending request times out at exactly 20 s, driven by a fake [`Clock`].
#[test]
fn correlation_times_out_after_20s() {
    let clock = TestClock::at(0);
    let mut correlator = Correlator::new();
    correlator.register(774, "/ci/authentication", clock.now_ms());

    clock.set(19_999);
    assert!(
        correlator.timed_out(clock.now_ms()).is_empty(),
        "must not time out one millisecond early"
    );

    clock.set(20_000);
    assert_eq!(
        correlator.timed_out(clock.now_ms()),
        vec![774],
        "must time out at exactly 20 s (message-model.md §5.3)"
    );
}
