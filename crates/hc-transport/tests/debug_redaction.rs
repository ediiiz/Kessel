//! [`Session`] embeds the FSM, whose notifications can carry device identifiers (deviceID, mac,
//! serialNumber). Its `Debug` delegates to the FSM's redacting impl, so none of them may appear in a
//! `{:?}`. The sentinels are first shown to be present in the routed corpus bodies.

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

use hc_proto::State;

use common::{client_session, corpus, corpus_raw, fake_wire};

/// Device identifiers that a routed body carries (`fixtures/aes/hob-session.json`): frame 3's
/// `/ci/registeredDevices` NOTIFY carries the deviceID and deviceName; frame 12's `/ci/info`
/// RESPONSE carries the mac and serialNumber.
const SENTINELS: &[&str] = &[
    "a4d7c797395d08be",            // deviceID (frame 3)
    "VV-N949X",                    // deviceName (frame 3)
    "IGEE-U94MH8TF1-38F49A1455A9", // deviceID / haId (frame 12)
    "38-F4-9A-14-55-A9",           // mac (frame 12)
    "258603004700104930",          // serialNumber (frame 12)
];

#[tokio::test]
async fn session_debug_never_renders_an_l10_identifier() {
    // Non-vacuity: these identifiers really are in the corpus bodies.
    let raw = corpus_raw();
    for sentinel in SENTINELS {
        assert!(
            raw.contains(sentinel),
            "sentinel {sentinel} must be present in a corpus body for this gate to mean anything"
        );
    }

    let corpus = corpus();
    let (sink, stream, device) = fake_wire();
    device.replay_rx(&corpus);
    let mut session = client_session(&corpus, sink, stream);
    let outcome = session
        .run_handshake()
        .await
        .expect("handshake completes against the fake device");
    assert_eq!(outcome.state, State::Connected);
    assert!(
        outcome.notifications >= 1,
        "at least the /ci/registeredDevices NOTIFY body was routed and is now reachable via the FSM"
    );

    for form in [format!("{session:?}"), format!("{session:#?}")] {
        // Squeeze whitespace so the pretty form cannot hide a match across a line break.
        let squeezed: String = form.chars().filter(|c| !c.is_whitespace()).collect();
        for sentinel in SENTINELS {
            assert!(
                !squeezed.contains(sentinel),
                "Session Debug leaked the appliance identifier {sentinel}: {form}"
            );
        }
    }

    drop(device);
}
