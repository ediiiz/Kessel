//! `sse_stream_filters_by_level_and_source`: `GET /api/events` filters on the server.
//!
//! The bus takes everything down to `TRACE` so the inspector is not gated behind `KESSEL_LOG`.
//! Filtering in the browser would still push every event over the wire, so level and subsystem are
//! applied here, before serialisation.
//!
//! A filtered stream has holes in `seq` by construction; the browser must not report them as drops
//! (see `detectGaps` in `apps/web/src/lib/frame-log.ts`). The server's half, asserted here, is that
//! the sent frames keep their original `seq` rather than being renumbered, so a genuine drop stays
//! detectable.
//!
//! An SSE body never ends, so `common::sse_body` reads with a deadline.

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

const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Five events across three subsystems and four levels, emitted before the request so the backlog
/// replay carries them: seq 0 `trace`/homeconnect, 1 `debug`/mqtt, 2 `info`/http, 3 `warn`/mqtt,
/// 4 `error`/homeconnect.
fn seeded() -> AppState {
    let state = AppState::new(common::bus());
    state.bus().emit(
        TraceRecord::new(TraceLevel::Trace, "hc_proto::socket", "a protocol frame")
            .with_field("direction", "rx"),
    );
    state.bus().emit(TraceRecord::new(
        TraceLevel::Debug,
        "server::mqtt",
        "a broker publish",
    ));
    state.bus().emit(TraceRecord::new(
        TraceLevel::Info,
        "axum::serve",
        "a connection accepted",
    ));
    state.bus().emit(TraceRecord::new(
        TraceLevel::Warn,
        "server::mqtt",
        "a broker reconnect",
    ));
    state.bus().emit(TraceRecord::new(
        TraceLevel::Error,
        "hc_transport::aes_ws",
        "a handshake failure",
    ));
    state
}

#[tokio::test]
async fn sse_stream_defaults_to_everything() {
    let (status, _headers, body) =
        common::sse_body(server::api::app(seeded()), "/api/events", 5, READ_TIMEOUT).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body.matches("data:").count(),
        5,
        "an unfiltered request is the firehose it was before the filter existed; body was:\n{body}"
    );
}

#[tokio::test]
async fn sse_stream_filters_by_level() {
    // `level=warn` means "warn and worse", the way every log threshold reads.
    let (status, _headers, body) = common::sse_body(
        server::api::app(seeded()),
        "/api/events?level=warn",
        2,
        READ_TIMEOUT,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body.matches("data:").count(),
        2,
        "only the `warn` and the `error` survive `level=warn`; body was:\n{body}"
    );
    assert!(
        body.contains("a broker reconnect") && body.contains("a handshake failure"),
        "the two events at or above `warn` are the two that arrive; body was:\n{body}"
    );
    assert!(
        !body.contains("a connection accepted"),
        "an `info` event must not survive `level=warn`; body was:\n{body}"
    );
    // The bus's numbering is preserved, so a genuine drop stays detectable.
    assert!(
        body.contains("id: 3") && body.contains("id: 4"),
        "the seq numbers are the bus's, not a renumbering of the filtered subset; body was:\n{body}"
    );
}

#[tokio::test]
async fn sse_stream_filters_by_source_group() {
    let (status, _headers, body) = common::sse_body(
        server::api::app(seeded()),
        "/api/events?source=mqtt",
        2,
        READ_TIMEOUT,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body.matches("data:").count(),
        2,
        "`server::mqtt` is the only target in the `mqtt` group here; body was:\n{body}"
    );
    assert!(
        body.contains("a broker publish") && body.contains("a broker reconnect"),
        "both mqtt events arrive regardless of level; body was:\n{body}"
    );
    assert!(
        !body.contains("a protocol frame"),
        "an `hc_proto` event is `homeconnect`, not `mqtt`; body was:\n{body}"
    );
}

#[tokio::test]
async fn sse_stream_accepts_several_source_groups() {
    let (status, _headers, body) = common::sse_body(
        server::api::app(seeded()),
        "/api/events?source=mqtt,homeconnect&level=debug",
        3,
        READ_TIMEOUT,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body.matches("data:").count(),
        3,
        "the two filters compose: mqtt+homeconnect at debug-or-worse drops the `axum` info line \
         and the `trace` frame; body was:\n{body}"
    );
    assert!(
        !body.contains("a connection accepted") && !body.contains("a protocol frame"),
        "body was:\n{body}"
    );
}

#[tokio::test]
async fn sse_stream_carries_the_source_group_on_every_frame() {
    let (_status, _headers, body) =
        common::sse_body(server::api::app(seeded()), "/api/events", 5, READ_TIMEOUT).await;

    // The UI groups by this field rather than keeping its own copy of the mapping.
    for expected in [
        r#""source":"homeconnect""#,
        r#""source":"mqtt""#,
        r#""source":"http""#,
    ] {
        assert!(
            body.contains(expected),
            "every frame carries the group its target belongs to, and `{expected}` is missing; \
             body was:\n{body}"
        );
    }
}

/// Pins the prefix table's two sharp edges: longest match wins, and matches end on a boundary.
#[test]
fn source_groups_match_on_a_path_boundary_longest_prefix_first() {
    use server::types::TraceSource;

    // Longest first: `server::mqtt` is the bridge, not the daemon's general lifecycle.
    assert_eq!(TraceSource::of_target("server::mqtt"), TraceSource::Mqtt);
    assert_eq!(
        TraceSource::of_target("server::mqtt::publish"),
        TraceSource::Mqtt
    );
    assert_eq!(TraceSource::of_target("server::api"), TraceSource::Daemon);

    // On a boundary: a target that merely starts with the letters of a group is not that group.
    assert_eq!(
        TraceSource::of_target("serverside::thing"),
        TraceSource::Other
    );
    assert_eq!(
        TraceSource::of_target("hc_transport::aes_ws"),
        TraceSource::HomeConnect
    );
    // Unmapped is `Other`, never a guess.
    assert_eq!(TraceSource::of_target("h2::codec"), TraceSource::Other);
}

#[tokio::test]
async fn sse_stream_rejects_an_unknown_filter_value() {
    // Silently ignoring it would show the operator a stream that is not the one they asked for.
    let reply = common::get(server::api::app(seeded()), "/api/events?level=chatty").await;
    assert_eq!(
        reply.status,
        StatusCode::BAD_REQUEST,
        "body: {}",
        reply.body
    );

    let reply = common::get(server::api::app(seeded()), "/api/events?source=telepathy").await;
    assert_eq!(
        reply.status,
        StatusCode::BAD_REQUEST,
        "body: {}",
        reply.body
    );
}
