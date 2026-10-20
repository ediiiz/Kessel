//! `Debug` redaction: no device identifier (`deviceID`, `mac`, `serialNumber`) from a decoded body
//! may appear in the `Debug` output of any public type that can reach one.
//!
//! The sentinels are first asserted present in the raw `data` field, so the test is not vacuous.
//! Both `{:?}` and `{:#?}` are checked, with whitespace squeezed out so the pretty form cannot hide
//! a match across line breaks.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use hc_crypto::{AesSession, PadSource, Role, SessionKeys};
use hc_proto::{
    Clock, DeviceIdentity, Driver, DriverSources, FakeSocket, Handshake, HandshakeConfig,
    IncomingMessage, NonceSource, parse,
};

/// Sentinels in the shape of the identifiers a real `/ci/info` body carries; none is a real value.
const DEVICE_ID: &str = "SENTINEL-DEVICEID-1a2b3c";
const MAC: &str = "SENTINEL-MAC-de:ad:be:ef";
const SERIAL: &str = "SENTINEL-SERIAL-99887766";

/// A `/ci/info`-shaped NOTIFY whose body carries the three identifiers.
fn ci_info_frame() -> String {
    format!(
        r#"{{"sID":1,"msgID":42,"resource":"/ci/info","version":1,"action":"NOTIFY","data":[{{"deviceID":"{DEVICE_ID}","mac":"{MAC}","serialNumber":"{SERIAL}"}}]}}"#
    )
}

fn sentinel_incoming() -> IncomingMessage {
    parse(ci_info_frame().as_bytes()).expect("the /ci/info frame parses")
}

fn config() -> HandshakeConfig {
    HandshakeConfig {
        identity: DeviceIdentity {
            name: "hcpy".to_owned(),
            id: "0badcafe".to_owned(),
        },
    }
}

#[derive(Debug)]
struct FixedNonce;
impl NonceSource for FixedNonce {
    fn next_nonce(&mut self) -> String {
        "test-nonce".to_owned()
    }
}

#[derive(Debug)]
struct ZeroClock;
impl Clock for ZeroClock {
    fn now_ms(&self) -> u64 {
        0
    }
}

#[derive(Debug)]
struct ZeroPad;
impl PadSource for ZeroPad {
    fn fill_pad(&mut self, out: &mut [u8]) {
        out.fill(0);
    }
}

/// A dummy 32-byte psk / 16-byte iv — the crypto is never exercised here, only `AesSession`'s
/// `Debug` (which `hc-crypto` already redacts). Not appliance key material.
fn dummy_keys() -> SessionKeys {
    SessionKeys::from_bytes(&[0_u8; 32], &[0_u8; 16]).expect("32-byte psk and 16-byte iv are valid")
}

/// A handshake driven just far enough to hold the sentinel `/ci/info` message in `notifications`:
/// after `POST /ei/initialValues`, any message that is not the `/ci/services` RESPONSE routes to the
/// general handler.
fn handshake_with_sentinel_notification() -> Handshake {
    let mut fsm = Handshake::new(config());
    let init = parse(
        br#"{"sID":1,"msgID":1,"resource":"/ei/initialValues","version":2,"action":"POST","data":[{"edMsgID":100}]}"#,
    )
    .expect("the initialValues frame parses");
    fsm.on_message(init, &mut FixedNonce)
        .expect("initialValues advances the FSM to AwaitingServices");
    fsm.on_message(sentinel_incoming(), &mut FixedNonce)
        .expect("a NOTIFY routes to the general handler");
    fsm
}

fn squeeze(rendered: &str) -> String {
    rendered.chars().filter(|c| !c.is_whitespace()).collect()
}

/// No identifier is reachable through the `Debug` of `IncomingMessage`, `Handshake` or `Driver`, in
/// either format.
#[test]
fn public_debug_never_renders_an_appliance_identifier() {
    let sentinels = [DEVICE_ID, MAC, SERIAL];

    let incoming = sentinel_incoming();
    // The raw `data` field, read directly, holds all three identifiers.
    let raw_data = format!("{:?}", incoming.data);
    for needle in sentinels {
        assert!(
            raw_data.contains(needle),
            "the raw data field must hold {needle} or this test is vacuous"
        );
    }

    let fsm = handshake_with_sentinel_notification();
    // The sentinel message really is in `notifications`.
    assert!(
        fsm.notifications()
            .iter()
            .any(|message| message.resource.as_deref() == Some("/ci/info")),
        "the sentinel /ci/info message must be in notifications or this test is vacuous"
    );

    let driver = Driver::new(
        FakeSocket::new(Vec::new()),
        AesSession::new(dummy_keys(), Role::Client),
        DriverSources {
            clock: ZeroClock,
            nonce: FixedNonce,
            pad: ZeroPad,
        },
        handshake_with_sentinel_notification(),
    );

    // Every public type that can transitively reach a decoded body belongs in this list.
    let renderings = [
        ("IncomingMessage", format!("{incoming:?}")),
        ("IncomingMessage alternate", format!("{incoming:#?}")),
        ("Handshake", format!("{fsm:?}")),
        ("Handshake alternate", format!("{fsm:#?}")),
        ("Driver", format!("{driver:?}")),
        ("Driver alternate", format!("{driver:#?}")),
    ];

    for (label, rendered) in &renderings {
        let squeezed = squeeze(rendered);
        for needle in sentinels {
            assert!(
                !squeezed.contains(needle),
                "{label} Debug leaked the appliance identifier {needle}:\n{rendered}"
            );
        }
    }
}
