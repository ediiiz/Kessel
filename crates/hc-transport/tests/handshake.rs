//! The async transport drives the handshake FSM to CONNECTED against an in-process fake device
//! replaying `fixtures/aes/hob-session.json`. No appliance is contacted.

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

use hc_proto::State;

use common::{client_session, corpus, fake_wire};

#[tokio::test]
async fn connects_and_completes_handshake_against_fake_device() {
    let corpus = corpus();
    let (sink, stream, device) = fake_wire();

    // The device replays the recorded rx frames verbatim, in receive order (0,3,4,11,12,13,14,15).
    device.replay_rx(&corpus);

    let mut session = client_session(&corpus, sink, stream);
    let outcome = tokio::time::timeout(Duration::from_secs(5), session.run_handshake())
        .await
        .expect("the handshake must complete, not hang")
        .expect("the handshake completes without error");

    assert_eq!(
        outcome.state,
        State::Connected,
        "the transport drives the FSM all the way to CONNECTED"
    );
    assert!(
        !outcome.saw_403,
        "no handshake response carried code 403 in the replay"
    );
    assert!(
        outcome.notifications >= 1,
        "the unsolicited /ci/registeredDevices NOTIFY was routed to the general handler"
    );

    // Keep the device (and thus the client's sink target) alive until the assertions are done.
    drop(device);
}
