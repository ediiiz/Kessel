//! A reconnect builds a fresh session with zeroed chains; nothing carries across.
//!
//! The production reconnect path is drop the `Session` and `connect()` again, which constructs fresh
//! `Sealer`/`Opener`. The test runs a first session to CONNECTED, drops it, reconnects over a fresh
//! fake device, and opens the new session's tx frames with a fresh appliance-role mirror: a carried
//! chain would fail on the first frame with `HmacMismatch`. No appliance is contacted.

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

use hc_crypto::{MAC_LEN, Opener, Role};
use hc_proto::State;
use hc_transport::WsFrame;
use hc_transport::aes_ws::{AesDecoder, AesEncoder};

use common::{client_session, corpus, fake_wire, test_pad};

const ZERO: [u8; MAC_LEN] = [0_u8; MAC_LEN];

#[tokio::test]
async fn reconnect_creates_fresh_session_and_zeroes_chains() {
    let corpus = corpus();

    // --- First connection: run to CONNECTED, advancing both chains, then drop it. ---
    let (sink1, stream1, device1) = fake_wire();
    device1.replay_rx(&corpus);
    let mut session1 = client_session(&corpus, sink1, stream1);
    let out1 = session1
        .run_handshake()
        .await
        .expect("the first handshake completes");
    assert_eq!(out1.state, State::Connected);
    drop(session1);
    drop(device1);

    // A freshly-built codec pair — exactly what `connect()` constructs on reconnect — starts zeroed
    // in both directions.
    let fresh_encoder = AesEncoder::new(corpus.keys(), test_pad());
    let fresh_decoder = AesDecoder::new(corpus.keys());
    assert_eq!(
        fresh_encoder.last_hmac(),
        ZERO,
        "a reconnect's fresh codec starts with a zeroed TX chain"
    );
    assert_eq!(
        fresh_decoder.last_hmac(),
        ZERO,
        "a reconnect's fresh codec starts with a zeroed RX chain"
    );

    // --- Reconnect: a fresh session over a fresh fake device, full handshake again. ---
    let (sink2, stream2, mut device2) = fake_wire();
    device2.replay_rx(&corpus);
    let mut session2 = client_session(&corpus, sink2, stream2);
    let out2 = session2
        .run_handshake()
        .await
        .expect("the reconnected handshake completes independently of the first");
    assert_eq!(out2.state, State::Connected);

    // Drop the session so its sink closes and the buffered tx frames drain to a clean channel end.
    drop(session2);

    // The reconnected session's tx frames must decrypt under a FRESH appliance mirror opening from a
    // zeroed chain — proof the reconnect started from zero and carried nothing across.
    let mut fresh_mirror = Opener::new(corpus.keys(), Role::Appliance.open_direction());
    let mut opened = 0_usize;
    while let Some(frame) = device2.recv().await {
        match frame {
            WsFrame::Binary(bytes) => {
                fresh_mirror
                    .open(&bytes)
                    .expect("reconnected tx frame decrypts from a zeroed chain");
                opened += 1;
            }
            WsFrame::Close => break,
            other => panic!("unexpected client frame {other:?}"),
        }
    }
    assert!(
        opened >= 1,
        "the reconnected session sent at least one frame decryptable from zero"
    );
}
