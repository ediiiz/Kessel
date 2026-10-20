//! Handshake state machine. Uses the captured corpus where it covers the case (ei v2, deviceReady)
//! and synthetic frames otherwise (ei v1, ci >= 3, response codes).

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

use common::corpus;
use hc_proto::{
    Action, DeviceIdentity, Handshake, HandshakeConfig, IncomingMessage, NonceSource,
    OutgoingMessage, State, parse,
};

/// Feeds the corpus opening frames (POST initialValues, then RESPONSE /ci/services) so a test can
/// inspect the request burst. Returns the burst emitted on the services response.
fn advance_to_burst(fsm: &mut Handshake) -> Vec<OutgoingMessage> {
    let corpus = corpus();
    fsm.on_message(parse(&corpus.frame(0).plaintext()).unwrap(), &mut nonce())
        .unwrap();
    fsm.on_message(parse(&corpus.frame(4).plaintext()).unwrap(), &mut nonce())
        .unwrap()
}

/// A synthetic RESPONSE carrying a `code`, for the tolerance tests (no corpus frame carries one).
fn response_with_code(resource: &str, code: i64) -> IncomingMessage {
    let frame = format!(
        r#"{{"sID":2879624297,"msgID":1,"resource":"{resource}","version":1,"action":"RESPONSE","code":{code}}}"#
    );
    parse(frame.as_bytes()).unwrap()
}

/// A fixed nonce source; these tests do not need byte-exact nonces.
struct FixedNonce(&'static str);
impl NonceSource for FixedNonce {
    fn next_nonce(&mut self) -> String {
        self.0.to_owned()
    }
}
const fn nonce() -> FixedNonce {
    FixedNonce("test-nonce")
}

fn config() -> HandshakeConfig {
    HandshakeConfig {
        identity: DeviceIdentity {
            name: "hcpy".to_owned(),
            id: "0badcafe".to_owned(),
        },
    }
}

fn wire_of(message: &OutgoingMessage) -> String {
    String::from_utf8(message.to_wire_bytes().unwrap()).unwrap()
}

/// On ei v2 `deviceType` is the string `"Application"` (corpus frame 0 is ei v2).
#[test]
fn device_type_is_string_application_on_ei_v2() {
    let corpus = corpus();
    let init = parse(&corpus.frame(0).plaintext()).unwrap();
    let mut fsm = Handshake::new(config());
    let out = fsm.on_message(init, &mut nonce()).unwrap();
    let response = out
        .first()
        .expect("RESPONSE /ei/initialValues emitted first");
    let text = wire_of(response);
    assert!(
        text.contains(r#""deviceType":"Application""#),
        "ei v2 -> the STRING \"Application\" (handshake.md §3): {text}"
    );
}

/// On ei v1 `deviceType` is the integer `2`.
#[test]
fn device_type_is_int_2_on_ei_v1() {
    let init = parse(
        br#"{"sID":1,"msgID":10,"resource":"/ei/initialValues","version":1,"action":"POST","data":[{"edMsgID":100}]}"#,
    )
    .unwrap();
    let mut fsm = Handshake::new(config());
    let out = fsm.on_message(init, &mut nonce()).unwrap();
    let text = wire_of(out.first().expect("RESPONSE emitted"));
    assert!(
        text.contains(r#""deviceType":2"#),
        "ei v1 -> the INT 2 (handshake.md §3): {text}"
    );
    assert!(
        !text.contains(r#""deviceType":"2""#),
        "must be the int 2, not the string \"2\" — the class of bug behind the 403 regression (handshake.md §11): {text}"
    );
}

/// `/ei/deviceReady` is sent unconditionally after initialValues.
#[test]
fn sends_device_ready_after_initial_values() {
    let corpus = corpus();
    let mut fsm = Handshake::new(config());

    let first = fsm
        .on_message(parse(&corpus.frame(0).plaintext()).unwrap(), &mut nonce())
        .unwrap();
    let response = first.first().expect("initialValues RESPONSE first");
    assert_eq!(response.resource, "/ei/initialValues");
    assert_eq!(response.action, Action::Response);

    let burst = fsm
        .on_message(parse(&corpus.frame(4).plaintext()).unwrap(), &mut nonce())
        .unwrap();
    let device_ready = burst
        .iter()
        .find(|message| message.resource == "/ei/deviceReady")
        .expect("deviceReady is sent unconditionally after initialValues (hcpy HCDevice.py:404)");
    assert_eq!(
        device_ready.action,
        Action::Notify,
        "deviceReady is a fire-and-forget NOTIFY"
    );
}

/// `/ci/authentication` and `/ci/info` are skipped when `ci >= 3`.
#[test]
fn skips_ci_authentication_when_ci_version_ge_3() {
    let mut fsm = Handshake::new(config());
    fsm.on_message(parse(&corpus().frame(0).plaintext()).unwrap(), &mut nonce())
        .unwrap();
    let services = parse(
        br#"{"sID":2879624297,"msgID":773,"resource":"/ci/services","version":1,"action":"RESPONSE","data":[{"service":"ro","version":1},{"service":"ei","version":2},{"service":"ci","version":3},{"service":"ni","version":1}]}"#,
    )
    .unwrap();
    let burst = fsm.on_message(services, &mut nonce()).unwrap();
    let resources: Vec<&str> = burst
        .iter()
        .map(|message| message.resource.as_str())
        .collect();

    assert!(
        !resources.contains(&"/ci/authentication"),
        "ci>=3 must skip /ci/authentication: {resources:?}"
    );
    assert!(
        !resources.contains(&"/ci/info"),
        "ci>=3 must skip /ci/info: {resources:?}"
    );
    // deviceReady is still unconditional even when auth is skipped.
    assert!(
        resources.contains(&"/ei/deviceReady"),
        "deviceReady stays unconditional"
    );
}

/// A `code` on `/ci/info` is tolerated; the handshake continues.
#[test]
fn tolerates_ci_info_failure() {
    let mut fsm = Handshake::new(config());
    advance_to_burst(&mut fsm);

    let result = fsm.on_message(response_with_code("/ci/info", 500), &mut nonce());
    assert!(
        result.is_ok(),
        "a code on /ci/info must be tolerated (error-codes.md §3): {result:?}"
    );
    assert_ne!(
        fsm.state(),
        State::Failed,
        "the handshake continues past a /ci/info failure"
    );
}

/// A `code >= 500` on `/ro/allMandatoryValues` is tolerated.
#[test]
fn tolerates_500_on_all_mandatory_values() {
    let mut fsm = Handshake::new(config());
    advance_to_burst(&mut fsm);
    let result = fsm.on_message(
        response_with_code("/ro/allMandatoryValues", 500),
        &mut nonce(),
    );
    assert!(
        result.is_ok(),
        "500 on /ro/allMandatoryValues must be tolerated (error-codes.md): {result:?}"
    );
    assert_ne!(fsm.state(), State::Failed);
}

/// A `code >= 500` on `/ro/allDescriptionChanges` is tolerated, across the 5xx band.
#[test]
fn tolerates_500_on_all_description_changes() {
    let mut fsm = Handshake::new(config());
    advance_to_burst(&mut fsm);
    let result = fsm.on_message(
        response_with_code("/ro/allDescriptionChanges", 503),
        &mut nonce(),
    );
    assert!(
        result.is_ok(),
        "503 on /ro/allDescriptionChanges must be tolerated (error-codes.md): {result:?}"
    );
    assert_ne!(fsm.state(), State::Failed);
}

/// A synthetic `NOTIFY /ro/values` carrying one item — the shape steady state actually delivers.
fn notify_value(msg_id: i64, uid: u64, value: i64) -> IncomingMessage {
    let frame = format!(
        r#"{{"sID":2879624297,"msgID":{msg_id},"resource":"/ro/values","version":1,"action":"NOTIFY","data":[{{"uid":{uid},"value":{value}}}]}}"#
    );
    parse(frame.as_bytes()).unwrap()
}

/// The notification buffer is drainable: the drain returns what was buffered, and the buffer is
/// empty afterwards, so a long-lived connection does not retain every NOTIFY.
#[test]
fn the_notification_buffer_is_drained_by_its_reader() {
    let mut fsm = Handshake::new(config());
    advance_to_burst(&mut fsm);
    fsm.on_message(notify_value(9001, 0x0100, 1), &mut nonce())
        .unwrap();
    fsm.on_message(notify_value(9002, 0x0101, 2), &mut nonce())
        .unwrap();
    assert_eq!(
        fsm.notifications().len(),
        2,
        "both NOTIFYs were routed to the general handler"
    );

    let drained = fsm.take_notifications();

    assert_eq!(
        drained.len(),
        2,
        "the drain returns every buffered NOTIFY — a reader that gets nothing back cannot consume them"
    );
    assert_eq!(
        drained.iter().filter_map(|m| m.msg_id).collect::<Vec<_>>(),
        vec![9001, 9002],
        "in arrival order, and they are the messages that were buffered"
    );
    assert_eq!(
        fsm.notifications().len(),
        0,
        "THE MEMORY PROPERTY: what has been handed to the reader is no longer retained here"
    );

    // A second drain yields nothing: consumption is not repeatable.
    assert!(fsm.take_notifications().is_empty());
}

/// Drives the handshake to CONNECTED by answering everything the burst asked for.
fn connected() -> Handshake {
    let mut fsm = Handshake::new(config());
    let burst = advance_to_burst(&mut fsm);
    for request in burst.iter().filter(|m| m.action == Action::Get) {
        let frame = format!(
            r#"{{"sID":2879624297,"msgID":1,"resource":"{}","version":1,"action":"RESPONSE","data":[]}}"#,
            request.resource
        );
        fsm.on_message(parse(frame.as_bytes()).unwrap(), &mut nonce())
            .unwrap();
    }
    assert_eq!(fsm.state(), State::Connected, "the handshake completes");
    drop(fsm.take_notifications());
    fsm
}

/// Once connected, a refused write is the appliance's answer to one request, not a broken
/// connection: the session stays up and the response reaches the reader.
#[test]
fn a_refused_write_after_connect_keeps_the_session() {
    let mut fsm = connected();
    let snapshot = fsm.ro_values().len();

    let result = fsm.on_message(response_with_code("/ro/values", 400), &mut nonce());

    assert!(
        result.is_ok(),
        "a refused write must not fail the session: {result:?}"
    );
    assert_eq!(fsm.state(), State::Connected);
    let delivered = fsm.take_notifications();
    assert_eq!(delivered.len(), 1, "the response is handed to the reader");
    assert_eq!(delivered.first().map(|m| m.action), Some(Action::Response));
    assert_eq!(
        fsm.ro_values().len(),
        snapshot,
        "steady-state responses are not added to the connect-time snapshot"
    );
}
