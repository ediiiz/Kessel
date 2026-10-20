//! The SSE stream multiplexes event types on one connection.
//!
//! One stream keeps the UI under the HTTP/1.1 per-origin connection cap. A browser's `EventSource`
//! dispatches on the SSE `event:` field, so this asserts the wire field, not a payload `kind`.
//!
//! The taxonomy follows the fields `hc-core` emits: a `state` field marks a `ConnectionState`
//! transition (`hc_core::supervisor::emit_state`), a `direction` field marks a frame for the
//! inspector, and anything else is a log line.
//!
//! An SSE body never ends, so the harness reads with a deadline and a silent stream fails after
//! two seconds rather than hanging.

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

use std::time::Duration;

use axum::http::StatusCode;
use hc_core::trace::{TraceLevel, TraceRecord};
use server::state::AppState;

/// How long the reader waits for the three events. Generous: they are already in the ring buffer
/// when the request is made, so the real path takes microseconds.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

#[tokio::test]
async fn sse_stream_multiplexes_event_types() {
    let state = AppState::new(common::bus());

    // Three events of three kinds, emitted before the request so the backlog replay carries them.
    state.bus().emit(
        TraceRecord::new(TraceLevel::Info, "hc_core::supervisor", "connection state")
            .with_field("device", "cooktop")
            .with_field("state", "connected"),
    );
    state.bus().emit(
        TraceRecord::new(TraceLevel::Trace, "hc_proto::socket", "frame")
            .with_field("direction", "rx")
            .with_field("resource", "/ci/info"),
    );
    state.bus().emit(TraceRecord::new(
        TraceLevel::Info,
        "server::api",
        "just a log line",
    ));

    let (status, _headers, body) =
        common::sse_body(server::api::app(state), "/api/events", 3, READ_TIMEOUT).await;

    assert_eq!(status, StatusCode::OK, "the SSE route answers 200");
    assert_eq!(
        body.matches("data:").count(),
        3,
        "all three emitted events reach the stream; body was:\n{body}"
    );

    for expected in ["event: connection", "event: frame", "event: log"] {
        assert!(
            body.contains(expected),
            "the stream must carry `{expected}` — a browser's EventSource dispatches on the \
             `event:` field, not on the payload. Body was:\n{body}"
        );
    }

    // The `event:` field and the payload's `kind` must agree, or a UI reading either one is right
    // by accident.
    assert!(
        body.contains(r#""kind":"connection""#),
        "the payload's kind mirrors the event type; body was:\n{body}"
    );
}
