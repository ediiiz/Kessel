//! The frame inspector does not depend on log verbosity.
//!
//! An `EnvFilter` on the `tracing` registry would filter every sink at once, so the inspector would
//! show nothing unless the operator raised `KESSEL_LOG`. The filters are therefore per-layer, and
//! this asserts the property: with the stderr filter clamped to `error`, a `debug!` and a `trace!`
//! event still reach the bus the SSE route reads from.
//!
//! The filters are passed in rather than read from the environment: `std::env::set_var` is `unsafe`
//! in edition 2024, this crate forbids unsafe code, and mutating the process environment is a
//! cross-test hazard anyway.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

mod common;

use tracing_subscriber::EnvFilter;

#[test]
fn inspector_filter_is_independent_of_log_verbosity() {
    let bus = common::bus();
    // The operator wants a quiet terminal. That must not touch the inspector.
    let subscriber = server::observability::subscriber_with_filters(
        &bus,
        EnvFilter::new("error"),
        EnvFilter::new("trace"),
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::debug!(marker = "debug-event", "a debug event");
        tracing::trace!(marker = "trace-event", "a trace event");
        tracing::error!(marker = "error-event", "an error event");
    });

    let messages: Vec<String> = bus
        .recent()
        .into_iter()
        .map(|event| event.message)
        .collect();

    assert!(
        messages.iter().any(|message| message == "a debug event"),
        "a `debug!` event must reach the frame inspector even with stderr clamped to `error` — \
         otherwise the inspector is gated behind KESSEL_LOG. Bus held: {messages:?}"
    );
    assert!(
        messages.iter().any(|message| message == "a trace event"),
        "a `trace!` event must reach the frame inspector: the per-frame firehose is exactly what \
         the inspector is for. Bus held: {messages:?}"
    );
    assert!(
        messages.iter().any(|message| message == "an error event"),
        "an `error!` event must reach the inspector too. Bus held: {messages:?}"
    );
    assert_eq!(
        bus.emitted(),
        3,
        "the inspector's own filter admitted all three levels"
    );
}
