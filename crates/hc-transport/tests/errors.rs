//! Fatal-frame handling: the AES transport is binary-only and drops the socket on a non-binary
//! frame, and a single bad rx frame yields exactly one error and then closes. No appliance is
//! contacted.

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

/// A binary frame that passes the length gates (32 bytes, block-aligned) but whose MAC cannot match
/// the RX chain — a deterministic `HmacMismatch`.
fn bad_binary_frame() -> WsFrame {
    WsFrame::Binary(vec![0_u8; 32])
}

#[tokio::test]
async fn non_binary_frame_drops_socket() {
    let corpus = corpus();
    let (sink, stream, device) = fake_wire();

    // A text data frame is illegal on the AES transport: binary only.
    device.send(WsFrame::Text("{\"action\":\"NOTIFY\"}".to_owned()));

    let mut session = client_session(&corpus, sink, stream);
    let result = tokio::time::timeout(Duration::from_secs(5), session.run_handshake())
        .await
        .expect("must not hang");

    assert!(
        matches!(result, Err(TransportError::UnexpectedFrame("text"))),
        "a non-binary data frame is an UnexpectedFrame that drops the socket, got {result:?}"
    );

    drop(device);
}

#[tokio::test]
async fn one_bad_rx_frame_yields_exactly_one_error_then_close() {
    let corpus = corpus();
    let (sink, stream, mut device) = fake_wire();

    // Start the handshake normally, then inject one bad frame mid-stream.
    let first_rx = corpus.rx_wire().first().cloned().expect("a first rx frame");
    device.send_binary(first_rx); // POST /ei/initialValues — kicks off the client
    device.send(bad_binary_frame()); // one bad rx frame

    let mut session = client_session(&corpus, sink, stream);
    let result = tokio::time::timeout(Duration::from_secs(5), session.run_handshake())
        .await
        .expect("must not hang");

    // Exactly one error: run_handshake returns a single Err, and it is the crypto hard error.
    assert!(
        matches!(result, Err(TransportError::Crypto(_))),
        "a bad frame yields one crypto error, got {result:?}"
    );

    // The client sent its two handshake frames (RESPONSE /ei/initialValues, GET /ci/services) in
    // response to frame 0, then hit the bad frame and closed. No flood: exactly a Close follows.
    let mut binary_before_close = 0_usize;
    let mut saw_close = false;
    while let Some(frame) = device.recv_within(500).await {
        match frame {
            WsFrame::Binary(_) => binary_before_close += 1,
            WsFrame::Close => {
                saw_close = true;
                break;
            }
            other => panic!("unexpected client frame {other:?}"),
        }
    }

    assert!(
        saw_close,
        "the socket is closed after the single error (a flood or a silent drop fails here)"
    );
    assert_eq!(
        binary_before_close, 2,
        "only the two handshake frames were sent before the close; the bad frame was not re-read"
    );
}
