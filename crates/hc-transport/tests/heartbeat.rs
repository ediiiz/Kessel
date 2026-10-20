//! Steady-state heartbeat: a WS ping every 20 s, failing the connection if a pong does not arrive
//! before the next interval. Driven by tokio's paused clock against the fake device.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed asserts; library code may not"
    )
)]

mod common;

use std::time::Duration;

use hc_transport::TransportError;
use hc_transport::WsFrame;
use hc_transport::session::HEARTBEAT_INTERVAL_MS;

use common::{client_session, corpus, fake_wire};

#[tokio::test(start_paused = true)]
async fn heartbeat_pings_every_20s_and_fails_on_pong_deadline() {
    let corpus = corpus();
    let (sink, stream, mut device) = fake_wire();
    let mut session = client_session(&corpus, sink, stream);

    let handle = tokio::spawn(async move { session.run_steady_state().await });

    // Let the task register its interval at t0, then advance to the first 20 s tick.
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(HEARTBEAT_INTERVAL_MS)).await;

    let first = device.recv().await;
    assert!(
        matches!(first, Some(WsFrame::Ping(_))),
        "a ping is sent at the first 20s interval, got {first:?}"
    );

    // The device withholds the pong; the next interval is the deadline.
    tokio::time::advance(Duration::from_millis(HEARTBEAT_INTERVAL_MS)).await;

    let result = handle.await.expect("the steady-state task joins");
    assert!(
        matches!(result, Err(TransportError::PongTimeout)),
        "a missed pong fails the connection with PongTimeout, got {result:?}"
    );
}
