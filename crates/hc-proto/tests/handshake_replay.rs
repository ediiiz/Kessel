//! Replays the golden AES corpus through a fake [`hc_proto::Socket`] and drives the FSM to
//! CONNECTED, checking every emitted frame byte-for-byte.
//!
//! A canned replay cannot produce a 403, so `ci_services_never_403_in_replay` only proves the
//! emitted frames match the captured ones; only a live appliance can show it will not 403.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "tests may panic on failed assertions; indexing into known corpus positions and \
                  the pad stream is a test-only invariant"
    )
)]

mod common;

use common::{Corpus, corpus};
use hc_crypto::{AesSession, PadSource, Role};
use hc_proto::{
    Action, Clock, DeviceIdentity, Driver, DriverSources, FakeSocket, Handshake, HandshakeConfig,
    NonceSource, State, parse,
};

/// A time source fixed at 0; the replay needs no real timeout.
struct ZeroClock;
impl Clock for ZeroClock {
    fn now_ms(&self) -> u64 {
        0
    }
}

/// Streams the recorded pad middles in tx order, so each `seal` reproduces the corpus ciphertext
/// byte-for-byte.
struct PadStream {
    bytes: Vec<u8>,
    pos: usize,
}
impl PadSource for PadStream {
    fn fill_pad(&mut self, out: &mut [u8]) {
        let end = self.pos + out.len();
        out.copy_from_slice(&self.bytes[self.pos..end]);
        self.pos = end;
    }
}

/// Yields the recorded `/ci/authentication` nonce token verbatim, so that tx frame is byte-exact.
struct RecordedNonce(String);
impl NonceSource for RecordedNonce {
    fn next_nonce(&mut self) -> String {
        self.0.clone()
    }
}

fn config() -> HandshakeConfig {
    HandshakeConfig {
        identity: DeviceIdentity {
            name: "hcpy".to_owned(),
            id: "0badcafe".to_owned(),
        },
    }
}

/// The captured `/ci/authentication` nonce token (corpus frame 5), carried verbatim.
fn recorded_nonce_token(corpus: &Corpus) -> String {
    corpus.frame(5).json["data"][0]["nonce"]
        .as_str()
        .expect("frame 5 carries a nonce")
        .to_owned()
}

/// A driver wired to the corpus: rx frames queued for `recv`, tx pad middles streamed in order, the
/// recorded nonce injected, the client-role AES chain, and the FSM.
fn corpus_driver() -> Driver<FakeSocket, ZeroClock, RecordedNonce, PadStream> {
    let corpus = corpus();
    let rx: Vec<Vec<u8>> = corpus
        .frames
        .iter()
        .filter(|frame| !frame.is_tx())
        .map(common::Frame::wire)
        .collect();
    let pad_bytes: Vec<u8> = corpus
        .frames
        .iter()
        .filter(|frame| frame.is_tx())
        .flat_map(common::Frame::pad_middle)
        .collect();
    let sources = DriverSources {
        clock: ZeroClock,
        nonce: RecordedNonce(recorded_nonce_token(&corpus)),
        pad: PadStream {
            bytes: pad_bytes,
            pos: 0,
        },
    };
    Driver::new(
        FakeSocket::new(rx),
        AesSession::new(corpus.keys(), Role::Client),
        sources,
        Handshake::new(config()),
    )
}

/// The unsolicited `NOTIFY /ci/registeredDevices` (frame 3) arrives between `GET /ci/services` and
/// its RESPONSE, and must route to the general handler.
#[test]
fn notify_routes_to_general_handler() {
    let mut driver = corpus_driver();
    driver
        .run_handshake()
        .expect("the golden corpus drives without error");

    let routed = driver.handshake().notifications().iter().any(|message| {
        message.action == Action::Notify
            && message.resource.as_deref() == Some("/ci/registeredDevices")
    });
    assert!(
        routed,
        "the unsolicited NOTIFY /ci/registeredDevices must reach the general handler"
    );
}

/// The FSM drives to CONNECTED against the fake socket, and every tx frame is byte-identical to the
/// captured frame, in order.
#[test]
fn handshake_reaches_connected_over_golden_aes_corpus() {
    let mut driver = corpus_driver();
    let outcome = driver
        .run_handshake()
        .expect("the golden corpus drives without error");

    assert_eq!(
        outcome.state,
        State::Connected,
        "the FSM must reach CONNECTED over the golden corpus"
    );

    let corpus = corpus();
    let expected: Vec<Vec<u8>> = corpus
        .frames
        .iter()
        .filter(|frame| frame.is_tx())
        .map(common::Frame::wire)
        .collect();
    assert_eq!(
        driver.socket().sent(),
        expected.as_slice(),
        "every tx frame must be byte-identical to the captured hcpy frame, in order"
    );
}

/// Guards against the `/ci/services` 403 by checking the emitted frames, chiefly an order-sensitive
/// byte comparison of the `/ei/initialValues` RESPONSE.
#[test]
fn ci_services_never_403_in_replay() {
    let corpus = corpus();
    let mut driver = corpus_driver();
    let outcome = driver
        .run_handshake()
        .expect("the golden corpus drives without error");

    // (1) No 403 was seen during the handshake (see the module docs for what this does not prove).
    assert!(
        !outcome.saw_403,
        "no handshake response carried code 403 in the replay"
    );
    assert_eq!(outcome.state, State::Connected);

    // Decrypt our own tx frames with an appliance-role opener, in send order, to inspect them.
    let mut opener = AesSession::new(corpus.keys(), Role::Appliance);
    let response = opener.open(&driver.socket().sent()[0]).unwrap();
    let services = opener.open(&driver.socket().sent()[1]).unwrap();

    // (2) The RESPONSE, compared on bytes: `serde_json::Value` equality ignores key order, and the
    // firmware does not.
    assert_eq!(
        response.plaintext.as_bytes(),
        corpus.frame(1).plaintext().as_slice(),
        "the /ei/initialValues RESPONSE must be byte-identical to hcpy's, key order included"
    );
    // The load-bearing key order and JSON types, spelled out.
    assert!(
        response.plaintext.contains(
            r#""data":[{"deviceType":"Application","deviceName":"hcpy","deviceID":"0badcafe"}]"#
        ),
        "data[0] must be deviceType(string),deviceName,deviceID in that order: {}",
        response.plaintext
    );
    assert!(
        response
            .plaintext
            .starts_with(r#"{"sID":2879624297,"msgID":1474258175,"resource":"/ei/initialValues","version":2,"action":"RESPONSE","data":"#),
        "top-level key order and echoed sID/msgID/version: {}",
        response.plaintext
    );
    assert!(
        !response.plaintext.contains("\"code\""),
        "the RESPONSE carries no code"
    );

    // (3) deviceID is stable across a fresh reconnect; a changing deviceID can provoke a 403.
    let mut again = corpus_driver();
    again.run_handshake().unwrap();
    assert_eq!(
        driver.socket().sent()[0],
        again.socket().sent()[0],
        "the emitted deviceID must be identical across runs — configured, never random"
    );

    // (4) The GET /ci/services request itself: version 1 (int), GET, no data, no code, msgID ==
    // the edMsgID from the captured init frame.
    let parsed = parse(services.plaintext.as_bytes()).unwrap();
    assert_eq!(parsed.action, Action::Get);
    assert_eq!(parsed.resource.as_deref(), Some("/ci/services"));
    assert_eq!(parsed.version.and_then(|version| version.as_int()), Some(1));
    assert!(parsed.data.is_none(), "GET /ci/services carries no data");
    assert!(parsed.code.is_none(), "GET /ci/services carries no code");
    assert_eq!(
        parsed.msg_id,
        Some(3_544_218_773),
        "msgID == edMsgID from the init frame"
    );
}

/// Regression pin for the handshake [`hc_proto::Body`] variants (`Empty`, `InitialValues`,
/// `Nonce`), which appliances accept byte-for-byte:
///
/// 1. each serialises byte-for-byte to its captured plaintext (frames 2, 1 and 5), and
/// 2. the whole FSM still drives to `CONNECTED` emitting all eight `tx` frames byte-identically.
///
/// It deliberately duplicates other assertions in this file.
#[test]
fn existing_handshake_bodies_are_unchanged() {
    let corpus = corpus();

    // (1a) Body::Empty — the `data` key is absent entirely, frame 2.
    let bare = hc_proto::OutgoingMessage::bare(
        corpus.frame(2).s_id,
        corpus.frame(2).msg_id,
        "/ci/services",
        hc_proto::Version::Int(1),
        Action::Get,
    );
    assert_eq!(
        bare.to_wire_bytes().unwrap(),
        corpus.frame(2).plaintext(),
        "Body::Empty must still serialise byte-for-byte to captured frame 2"
    );

    // (1b) Body::InitialValues — deviceType,deviceName,deviceID in that order, frame 1.
    let response = hc_proto::OutgoingMessage::initial_values_response(
        corpus.frame(1).s_id,
        corpus.frame(1).msg_id,
        hc_proto::Version::Int(2),
        hc_proto::DeviceType::Application,
        &config().identity,
    );
    assert_eq!(
        response.to_wire_bytes().unwrap(),
        corpus.frame(1).plaintext(),
        "Body::InitialValues must still serialise byte-for-byte to captured frame 1"
    );

    // (1c) Body::Nonce — `data":[{"nonce":…}]`, frame 5.
    let authentication = hc_proto::OutgoingMessage::authentication(
        corpus.frame(5).s_id,
        corpus.frame(5).msg_id,
        hc_proto::Version::Int(2),
        recorded_nonce_token(&corpus),
    );
    assert_eq!(
        authentication.to_wire_bytes().unwrap(),
        corpus.frame(5).plaintext(),
        "Body::Nonce must still serialise byte-for-byte to captured frame 5"
    );

    // (2) And the whole replay: CONNECTED, with all eight tx frames byte-identical and in order.
    let mut driver = corpus_driver();
    let outcome = driver
        .run_handshake()
        .expect("the golden corpus must still drive without error");
    assert_eq!(
        outcome.state,
        State::Connected,
        "the corpus replay must still reach CONNECTED"
    );
    let expected: Vec<Vec<u8>> = corpus
        .frames
        .iter()
        .filter(|frame| frame.is_tx())
        .map(common::Frame::wire)
        .collect();
    assert_eq!(
        expected.len(),
        8,
        "the corpus carries eight tx frames; a change here means the fixture moved"
    );
    assert_eq!(
        driver.socket().sent(),
        expected.as_slice(),
        "all eight tx frames must remain byte-identical to the captured hcpy frames"
    );
}
