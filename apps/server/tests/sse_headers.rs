//! The SSE route disables response buffering.
//!
//! A buffering reverse proxy (such as Home Assistant ingress) would turn the live frame inspector
//! into a stream that delivers nothing until the connection closes, while the endpoint still
//! answers 200. This asserts `X-Accel-Buffering: no`, the de-facto header nginx and several other
//! proxies honour, plus `Cache-Control: no-cache`, which stops an intermediary treating the stream
//! as a cacheable body. Whether Home Assistant's aiohttp-based proxy honours the former is not
//! tested here.
//!
//! The headers are asserted without reading the body: an SSE body never ends.

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

use axum::body::Body;
use axum::http::{Request, StatusCode};
use server::state::AppState;

#[tokio::test]
async fn sse_disables_buffering_header() {
    let state = AppState::new(common::bus());
    let request = Request::builder()
        .uri("/api/events")
        .body(Body::empty())
        .expect("a well-formed request");

    let (status, headers) = common::send_headers_only(server::api::app(state), request).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "the SSE route must answer 200 before the first event"
    );
    assert_eq!(
        headers
            .get("x-accel-buffering")
            .and_then(|value| value.to_str().ok()),
        Some("no"),
        "the SSE route must disable proxy buffering, or events stall behind a proxy; headers were {headers:?}"
    );
    assert_eq!(
        headers
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-cache"),
        "an SSE stream must not be cached; headers were {headers:?}"
    );
    assert!(
        headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream")),
        "the SSE route must be `text/event-stream`; headers were {headers:?}"
    );
}
