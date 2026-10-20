//! Connect and handshake over a real in-process TLS-PSK tunnel. The client connects through the
//! vendored OpenSSL, WS-upgrades over the tunnel, and drives the shared
//! [`hc_transport::session::Session`] to CONNECTED. The server replays the `plaintext_utf8` of
//! `fixtures/aes/hob-session.json` as text frames; the JSON/FSM sequence is transport-independent.
//! No appliance is contacted.

#![cfg(feature = "tls-psk")]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed asserts; library code may not"
    )
)]

mod tls_psk_support;

use std::time::Duration;

use hc_proto::State;
use hc_transport::tls_psk;

use tls_psk_support::{client_psk, corpus_identity, replay_handshake, spawn_loopback};

#[tokio::test]
async fn connects_and_drives_handshake_over_tls_psk_loopback() {
    let (addr, server) = spawn_loopback(replay_handshake).await;
    let host = addr.ip().to_string();
    let psk = client_psk();

    // The full production path: TLS-PSK connect -> WS upgrade over the tunnel -> shared Session.
    let mut session = tokio::time::timeout(
        Duration::from_secs(10),
        tls_psk::connect(&host, addr.port(), &psk, corpus_identity()),
    )
    .await
    .expect("connect must not hang")
    .expect("connect over the TLS-PSK loopback");

    let outcome = tokio::time::timeout(Duration::from_secs(10), session.run_handshake())
        .await
        .expect("the handshake must complete, not hang")
        .expect("the handshake completes without error");

    assert_eq!(
        outcome.state,
        State::Connected,
        "the TLS-PSK transport drives the shared FSM all the way to CONNECTED"
    );
    assert!(
        !outcome.saw_403,
        "no handshake response carried code 403 in the replay"
    );
    assert!(
        outcome.notifications >= 1,
        "the unsolicited /ci/registeredDevices NOTIFY was routed to the general handler"
    );

    drop(session);
    let _ = server.await;
}
