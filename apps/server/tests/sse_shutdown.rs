//! An open event stream ends when the daemon begins shutting down.
//!
//! The graceful shutdown waits for in-flight responses, and an SSE body never ends on its own, so
//! without this one open browser tab holds a stop or restart of the add-on until the Supervisor
//! kills it.

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

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use server::state::AppState;
use tower::ServiceExt as _;

#[tokio::test]
async fn shutdown_ends_an_open_event_stream() {
    let state = AppState::new(common::bus());
    let request = Request::builder()
        .uri("/api/events")
        .body(Body::empty())
        .expect("a well-formed request");
    let response = server::api::app(state.clone())
        .oneshot(request)
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::OK);

    state.begin_shutdown();

    let body = tokio::time::timeout(Duration::from_secs(2), response.into_body().collect()).await;
    assert!(
        body.is_ok_and(|collected| collected.is_ok()),
        "the event stream must end once shutdown begins, not hold the drain open"
    );
}
