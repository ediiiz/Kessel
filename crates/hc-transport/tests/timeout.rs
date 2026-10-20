//! The 20 s per-request timeout (`RESPONSE_TIMEOUT_MS`): a registered request whose response never
//! arrives yields `RequestTimeout` naming the unanswered resource, and the socket is closed cleanly.
//! Driven by tokio's paused clock. No appliance is contacted.

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

use hc_transport::{TransportError, WsFrame};

use common::{client_session, corpus, fake_wire};

#[tokio::test(start_paused = true)]
async fn request_timeout_fires_and_names_the_unanswered_request() {
    let corpus = corpus();
    let (sink, stream, mut device) = fake_wire();

    // Kick off the handshake, then withhold every further frame.
    let first_rx = corpus.rx_wire().first().cloned().expect("a first rx frame");
    device.send_binary(first_rx);

    let mut session = client_session(&corpus, sink, stream);
    let handle = tokio::spawn(async move { session.run_handshake().await });

    // The client consumes /ei/initialValues, emits the RESPONSE + GET /ci/services (registering the
    // GET for correlation), then parks awaiting the /ci/services response that never comes.
    let _response = device.recv().await.expect("RESPONSE /ei/initialValues");
    let _services_get = device.recv().await.expect("GET /ci/services");

    // Advance past the 20 s deadline on the paused clock.
    tokio::time::advance(Duration::from_millis(hc_proto::RESPONSE_TIMEOUT_MS + 1)).await;

    let result = handle.await.expect("the handshake task joins");
    match result {
        Err(TransportError::RequestTimeout { resource }) => {
            assert_eq!(
                resource, "/ci/services",
                "the timeout names the request that went unanswered"
            );
        }
        other => panic!("expected RequestTimeout, got {other:?}"),
    }

    // The socket is closed after the timeout, the same close-and-reconnect path as a crypto error.
    let closed = device.recv().await;
    assert!(
        matches!(closed, Some(WsFrame::Close)),
        "the socket is closed after the timeout, got {closed:?}"
    );
}
