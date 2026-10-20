//! The daemon's write path, end to end through the real HTTP route.
//!
//! Touches no device: every session is built over an in-process recording sink and a receive half
//! that reports the peer closed, and every `sID`/`msgID` is a synthetic per-connection number. The
//! profiles are the four scrubbed fixtures, with synthetic identifiers and a synthetic psk/iv.
//!
//! The tests drive the router rather than the registry, because a handler that built a fresh
//! `GuardedWriter` (and so a fresh `msgID` counter) per request would pass a registry-only test. Each
//! test issues real `POST /api/devices/{slug}/write` requests through `server::api::app` and asserts
//! on what reached the wire.
//!
//! Every refusal is asserted against the recorded byte count, not only the HTTP status: a writer
//! that transmitted first and authorized second would still return the correct error.
//!
//! `hob_and_oven_writes_reach_the_wire_through_the_http_route` asserts 200 and one frame at the sink
//! for both: every appliance is writable.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "tests may panic on failed assertions; indexing a JSON response panics loudly \
                  on a missing key"
    )
)]

mod common;

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use hc_core::bridge::take_guarded_session_boxed;
use hc_core::device::DeviceRole;
use hc_profile::Profile;
use hc_proto::{
    Action, DeviceIdentity, Handshake, HandshakeConfig, IncomingMessage, OutgoingMessage,
    RandomNonce, Version,
};
use hc_transport::{
    Endpoints, FrameDecoder, FrameEncoder, FrameSink, FrameStream, Session, TransportError, WsFrame,
};
use parking_lot::Mutex;
use server::auth::{ApiToken, TOKEN_HEADER};
use server::state::{AppState, DeviceEntry};
use server::write::LiveWriter;

/// The token these tests authorise with. Not a secret and not a real one.
const TEST_TOKEN: &str = "test-token-not-a-real-one";

/// Synthetic per-connection counters, not appliance identifiers.
const S_ID: u64 = 1;
const ED_MSG_ID: u64 = 1;
/// The `msgID` the synthetic appliance stamps on its opening `POST`; its RESPONSE reuses it and
/// spends no client id, so it must not collide with a client id.
const SERVER_MSG_ID: u64 = 900_000;

// ---------------------------------------------------------------------------------------------
// An in-process session. Nothing below opens a socket.
// ---------------------------------------------------------------------------------------------

/// Every frame that reached the sink, as raw bytes.
#[derive(Clone, Default)]
struct Wire {
    frames: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Wire {
    /// Total bytes written — `0` proves nothing left the process.
    fn byte_count(&self) -> usize {
        self.frames.lock().iter().map(Vec::len).sum()
    }

    /// The `msgID` of every captured frame, in send order. The frames are plaintext JSON here.
    fn msg_ids(&self) -> Vec<u64> {
        self.frames
            .lock()
            .iter()
            .map(|bytes| {
                hc_proto::parse(bytes)
                    .expect("every captured frame is parseable JSON")
                    .msg_id
                    .expect("every emitted frame carries a msgID")
            })
            .collect()
    }
}

struct RecordingSink {
    wire: Wire,
}

#[async_trait]
impl FrameSink for RecordingSink {
    async fn send_frame(&mut self, frame: WsFrame) -> Result<(), TransportError> {
        let bytes = match frame {
            WsFrame::Binary(bytes) => bytes,
            WsFrame::Text(text) => text.into_bytes(),
            other => return Err(TransportError::UnexpectedFrame(other.kind())),
        };
        self.wire.frames.lock().push(bytes);
        Ok(())
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        Ok(())
    }
}

/// A plaintext encoder: the message's own wire bytes, unsealed, so an assertion can read them. This
/// is the TLS-PSK framing (no app-layer crypto).
struct PlainEncoder;

impl FrameEncoder for PlainEncoder {
    fn encode(&mut self, message: &OutgoingMessage) -> Result<WsFrame, TransportError> {
        Ok(WsFrame::Binary(message.to_wire_bytes()?))
    }
}

/// The receive half: the peer is already gone, so nothing is ever received.
struct ClosedStream;

#[async_trait]
impl FrameStream for ClosedStream {
    async fn next_frame(&mut self) -> Result<WsFrame, TransportError> {
        Err(TransportError::Closed)
    }
}

struct EmptyDecoder {
    messages: VecDeque<IncomingMessage>,
}

impl FrameDecoder for EmptyDecoder {
    fn decode(&mut self, _frame: WsFrame) -> Result<IncomingMessage, TransportError> {
        self.messages.pop_front().ok_or(TransportError::Closed)
    }
}

/// A synthetic `POST /ei/initialValues`, the appliance's opening message.
fn initial_values(ed_msg_id: u64) -> IncomingMessage {
    IncomingMessage {
        s_id: Some(S_ID),
        msg_id: Some(SERVER_MSG_ID),
        resource: Some("/ei/initialValues".to_owned()),
        version: Some(Version::Int(2)),
        action: Action::Post,
        data: Some(vec![serde_json::json!({"edMsgID": ed_msg_id})]),
        code: None,
    }
}

/// A `Handshake` positioned exactly as a real connection would position it, driven outside any
/// session so no frame reaches a sink. Its own `GET /ci/services` spends `ed_msg_id`, so the first id
/// a writer spends is `ed_msg_id + 1`.
fn seeded_handshake(ed_msg_id: u64) -> Handshake {
    let mut fsm = Handshake::new(HandshakeConfig {
        // Synthetic identity: no real `deviceName`/`deviceID`.
        identity: DeviceIdentity {
            name: "kessel-test".to_owned(),
            id: "00000000".to_owned(),
        },
    });
    let _dropped = fsm
        .on_message(initial_values(ed_msg_id), &mut RandomNonce)
        .expect("a well-formed POST /ei/initialValues advances the FSM");
    fsm
}

type FakeSession = Session<RecordingSink, ClosedStream, PlainEncoder, EmptyDecoder>;

fn fake_session(wire: &Wire, ed_msg_id: u64) -> FakeSession {
    Session::new(
        Endpoints::new(
            RecordingSink { wire: wire.clone() },
            ClosedStream,
            PlainEncoder,
            EmptyDecoder {
                messages: VecDeque::new(),
            },
        ),
        seeded_handshake(ed_msg_id),
        Box::new(RandomNonce),
    )
}

/// Builds the writer a live connection would install, over an in-process session.
///
/// It goes through the same `hc_core::bridge` sealer production uses, so only the transport halves
/// differ from the writer the daemon installs.
fn installed_writer(ed_msg_id: u64) -> (LiveWriter, Wire) {
    let wire = Wire::default();
    // The session is dropped at the end of this function; the writer holds a `Sender` over the same
    // TX half, and a write does not need the session's receive loop.
    let mut session = fake_session(&wire, ed_msg_id);
    let writer = take_guarded_session_boxed(&mut session)
        .expect("a fresh session has not yet yielded its send handle");
    (writer, wire)
}

// ---------------------------------------------------------------------------------------------
// The daemon under test.
// ---------------------------------------------------------------------------------------------

/// A state with one onboarded device, from the named scrubbed fixture.
fn state_with(slug: &str, role: DeviceRole, fixture: &str) -> AppState {
    let state = AppState::new(common::bus()).with_token(ApiToken::from_string(TEST_TOKEN));
    let profile =
        Profile::from_zip_bytes(&common::fixture_zip_bytes(fixture)).expect("the fixture parses");
    state
        .devices()
        .insert(slug, DeviceEntry::from_profile(slug, role, &profile));
    state
}

/// A `POST /api/devices/{slug}/write` for one feature key.
fn write_request(slug: &str, feature_key: &str, value: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/api/devices/{slug}/write"))
        .header("content-type", "application/json")
        .header(TOKEN_HEADER, TEST_TOKEN)
        .body(Body::from(format!(
            r#"{{"feature_key":"{feature_key}","value":{value}}}"#
        )))
        .expect("a well-formed request")
}

/// The feature these tests write: named, not discovered.
///
/// `Time.DisplayMode` is a `readWrite` setting on the scrubbed dishwasher whose enum members are
/// `Relative` and `Absolute`; it only changes how a clock is rendered. The first writable feature the
/// profile declares is `BSH.Common.Command.DeactivateWiFi`, which must never be picked by accident.
const WRITE_FEATURE: &str = "BSH.Common.Setting.Time.DisplayMode";
/// A declared member of [`WRITE_FEATURE`]'s enumeration, as the UI would submit it.
const WRITE_VALUE: &str = r#""Absolute""#;

/// Two writes to one device reach the same writer, so their `msgID`s are one sequence.
///
/// A fresh writer per request would restart the counter, so both frames would carry the same id,
/// and on a real connection that id would land on a live correlation slot.
#[tokio::test]
async fn writer_registry_holds_one_writer_per_device() {
    let state = state_with("dishwasher", DeviceRole::Dishwasher, "dishwasher-tls.zip");
    let (writer, wire) = installed_writer(ED_MSG_ID);
    state.writers().install("dishwasher", writer);

    // Asserted separately so an empty registry is not mistaken for the no-session refusal.
    assert!(
        state.writers().writer("dishwasher").is_some(),
        "a writer was just installed for `dishwasher`, so the registry must hand it back; otherwise \
         every write silently takes the no-live-session fallback"
    );

    for attempt in 1..=2_u32 {
        let reply = common::send(
            server::api::app(state.clone()),
            write_request("dishwasher", WRITE_FEATURE, WRITE_VALUE),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "write {attempt} of 2 must be accepted — the registry's writer is installed and the \
             live-write gate is open on it: {}",
            reply.body
        );
    }

    assert_eq!(
        wire.msg_ids(),
        vec![ED_MSG_ID + 1, ED_MSG_ID + 2],
        "two writes to one device must spend two CONSECUTIVE msgIDs from the connection's own \
         allocator. Two equal ids mean a fresh GuardedWriter was built per request — the defect this \
         registry exists to fix — and on a real connection the second would collide with a live \
         correlation slot"
    );
    // When the guard allows a write, bytes really leave; otherwise the id assertion would be vacuous.
    assert!(
        wire.byte_count() > 0,
        "two accepted writes must have put bytes on the wire"
    );
}

/// A write to a device with no live session is refused and transmits nothing.
///
/// The transport is exercised directly as well as over HTTP: a test that checked only the HTTP status
/// would pass against a no-session transport that silently succeeded.
#[tokio::test]
async fn a_write_with_no_live_session_is_refused() {
    let state = state_with("dishwasher", DeviceRole::Dishwasher, "dishwasher-tls.zip");
    assert!(
        state.writers().is_empty(),
        "the registry starts empty — an empty registry IS the fail-closed state, which is why \
         AppState has no writer-injection seam"
    );

    let reply = common::send(
        server::api::app(state.clone()),
        write_request("dishwasher", WRITE_FEATURE, WRITE_VALUE),
    )
    .await;
    assert_ne!(
        reply.status,
        StatusCode::OK,
        "a write to a device with no live session must never be accepted: {}",
        reply.body
    );
    assert_eq!(
        reply.json()["outcome"],
        "refused",
        "and it is reported as a WriteOutcome refusal, not a queue or a 500: {}",
        reply.body
    );
    assert!(
        state.writers().is_empty(),
        "a refused write must not have installed anything"
    );

    // The other mechanism: with the guard's gates passed, the no-session transport itself refuses.
    // Reached directly because gate 2 fires first over HTTP in this process.
    let mut transport = server::write::no_live_session_transport();
    let command = hc_core::write::WriteCommand {
        resource: "/ro/values".to_owned(),
        target_uid: Some(0x0100),
        field: None,
        value: serde_json::json!(7),
        validate: true,
        options: Vec::new(),
    };
    let sent = hc_core::write::WriteTransport::transmit(&mut transport, &command).await;
    assert!(
        sent.is_err(),
        "the no-live-session transport must refuse even when the guard has allowed the write — \
         nothing is queued for a disconnected appliance: {sent:?}"
    );
}

/// A reconnect replaces the registry entry, and the new writer does not spend the dead
/// connection's ids.
///
/// A reconnect builds a new `Handshake` with a new `sID` and a `MsgIdAllocator` positioned at the new
/// `edMsgID`. A writer held across a reconnect would keep spending from the dead connection's cell,
/// which no frame count would reveal.
///
/// The two connections get far-apart `edMsgID`s, 1 and 100, so after the reconnect the next id must
/// be 101; a stale writer would emit 3 and a fresh writer per request would emit 2. This also pins
/// that `WriterRegistry::install` replaces rather than inserting if absent.
#[tokio::test]
async fn a_reconnect_replaces_the_registry_entry() {
    const RECONNECT_ED_MSG_ID: u64 = 100;

    let state = state_with("dishwasher", DeviceRole::Dishwasher, "dishwasher-tls.zip");

    // Connection 1.
    let (first, first_wire) = installed_writer(ED_MSG_ID);
    state.writers().install("dishwasher", first);
    let reply = common::send(
        server::api::app(state.clone()),
        write_request("dishwasher", WRITE_FEATURE, WRITE_VALUE),
    )
    .await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "the write on connection 1 must be accepted: {}",
        reply.body
    );

    // The connection drops and comes back with a new handshake, a new sID and an allocator at a new
    // edMsgID. This is what `live_transport`'s device task does on every reconnect.
    let (second, second_wire) = installed_writer(RECONNECT_ED_MSG_ID);
    state.writers().install("dishwasher", second);
    let reply = common::send(
        server::api::app(state.clone()),
        write_request("dishwasher", WRITE_FEATURE, WRITE_VALUE),
    )
    .await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "the write after the reconnect must be accepted: {}",
        reply.body
    );

    assert_eq!(
        second_wire.msg_ids(),
        vec![RECONNECT_ED_MSG_ID + 1],
        "the post-reconnect write must spend an id from the NEW connection's allocator. `3` would \
         mean the dead connection's writer was still installed; `2` would mean a fresh writer per \
         request"
    );
    assert_eq!(
        first_wire.msg_ids(),
        vec![ED_MSG_ID + 1],
        "and the dead connection's wire must be untouched by the second write — one frame, the one \
         it was sent before the reconnect"
    );
    assert_eq!(
        state.writers().len(),
        1,
        "a reconnect replaces the entry rather than accumulating one per connection"
    );
}

/// The hob and the oven reach the wire through the real HTTP route.
///
/// The request goes through `server::api::app`, the token gate, feature resolution,
/// `build_write_command`, the registry lookup and the writer a device task installs, and:
///
/// 1. the HTTP response is `200 {"outcome":"accepted"}`;
/// 2. exactly one frame appears at the sink, spending the session's own next `msgID`. An `accepted`
///    body with an empty sink would be a write that silently did nothing.
#[tokio::test]
async fn hob_and_oven_writes_reach_the_wire_through_the_http_route() {
    for (slug, role, fixture) in [
        ("cooktop", DeviceRole::Cooktop, "hob-aes.zip"),
        ("oven", DeviceRole::Oven, "oven-tls.zip"),
    ] {
        let state = state_with(slug, role.clone(), fixture);
        let (writer, wire) = installed_writer(ED_MSG_ID);
        state.writers().install(slug, writer);

        // The feature key comes from the appliance's own profile: a 404 on the key would make the
        // assertions below meaningless, because the write would never reach the writer.
        let feature = state
            .devices()
            .entry(slug)
            .expect("the device is registered")
            .entity_descriptors(None)
            .into_iter()
            .find(|descriptor| descriptor.writable)
            .and_then(|descriptor| descriptor.feature_key)
            .expect("both profiles declare a writable feature");

        let reply = common::send(
            server::api::app(state.clone()),
            write_request(slug, &feature, r#""On""#),
        )
        .await;

        assert_eq!(
            reply.status,
            StatusCode::OK,
            "a {slug} write is accepted through the real route: {}",
            reply.body
        );
        assert_eq!(
            reply.json()["outcome"],
            "accepted",
            "and reported as accepted: {}",
            reply.body
        );
        assert_eq!(
            wire.msg_ids().len(),
            1,
            "{slug}: exactly one frame reached the sink. Zero would mean an `accepted` body for a write \
             that did nothing"
        );
        assert_eq!(
            wire.msg_ids(),
            vec![ED_MSG_ID + 1],
            "{slug}: and it spends the connection's own next msgID"
        );
    }
}

/// The hob and the oven reach the wire through the MQTT route too.
///
/// The MQTT path has no axum router, no token gate and no `WriteRequest`, so it is asserted on its
/// own rather than argued from the HTTP test.
#[tokio::test]
async fn a_hob_set_message_reaches_the_wire_through_the_mqtt_route() {
    for (slug, role, fixture) in [
        ("cooktop", DeviceRole::Cooktop, "hob-aes.zip"),
        ("oven", DeviceRole::Oven, "oven-tls.zip"),
    ] {
        let state = state_with(slug, role.clone(), fixture);
        let (writer, wire) = installed_writer(ED_MSG_ID);
        state.writers().install(slug, writer);

        let entry = state.devices().entry(slug).expect("registered");
        let relative = entry
            .mapping
            .entries()
            .iter()
            .find_map(|mapped| mapped.command_topic.clone())
            .expect("both profiles declare a writable feature");
        let topic = format!(
            "{}/{slug}/{relative}",
            server::mqtt::MqttSettings::DEFAULT_BASE
        );

        let outcome = server::mqtt::route_inbound_set(
            &state,
            server::mqtt::MqttSettings::DEFAULT_BASE,
            slug,
            &topic,
            "On",
        )
        .await;

        assert!(
            matches!(outcome, server::mqtt::SetRouting::Accepted),
            "a {slug} set message is accepted: {outcome:?}"
        );
        assert_eq!(
            wire.msg_ids().len(),
            1,
            "{slug}: exactly one frame reached the sink through the MQTT route"
        );
        assert_eq!(wire.msg_ids(), vec![ED_MSG_ID + 1]);
    }
}

/// The dishwasher's set message reaches the wire too.
#[tokio::test]
async fn a_dishwasher_set_message_reaches_the_wire_through_the_mqtt_route() {
    let state = state_with("dishwasher", DeviceRole::Dishwasher, "dishwasher-tls.zip");
    let (writer, wire) = installed_writer(ED_MSG_ID);
    state.writers().install("dishwasher", writer);

    let entry = state.devices().entry("dishwasher").expect("registered");
    let relative = entry
        .mapping
        .entries()
        .iter()
        .find_map(|mapped| mapped.command_topic.clone())
        .expect("the dishwasher declares a writable feature");
    let topic = format!(
        "{}/dishwasher/{relative}",
        server::mqtt::MqttSettings::DEFAULT_BASE
    );

    let outcome = server::mqtt::route_inbound_set(
        &state,
        server::mqtt::MqttSettings::DEFAULT_BASE,
        "dishwasher",
        &topic,
        "On",
    )
    .await;

    assert!(
        matches!(outcome, server::mqtt::SetRouting::Accepted),
        "an allowlisted device's set message must reach the guard and be accepted: {outcome:?}"
    );
    assert_eq!(
        wire.msg_ids(),
        vec![ED_MSG_ID + 1],
        "exactly one frame, spending the connection's own next msgID — the same property the HTTP \
         route's test pins, now for the broker path"
    );
}

/// A set message for a device with no live session is refused, and nothing is queued.
#[tokio::test]
async fn a_set_message_for_a_disconnected_device_is_refused() {
    let state = state_with("dishwasher", DeviceRole::Dishwasher, "dishwasher-tls.zip");
    // No writer installed: this is a device whose task is not holding a connection.
    let entry = state.devices().entry("dishwasher").expect("registered");
    let relative = entry
        .mapping
        .entries()
        .iter()
        .find_map(|mapped| mapped.command_topic.clone())
        .expect("writable feature");
    let topic = format!(
        "{}/dishwasher/{relative}",
        server::mqtt::MqttSettings::DEFAULT_BASE
    );

    let outcome = server::mqtt::route_inbound_set(
        &state,
        server::mqtt::MqttSettings::DEFAULT_BASE,
        "dishwasher",
        &topic,
        "On",
    )
    .await;
    assert!(
        matches!(outcome, server::mqtt::SetRouting::NoLiveSession),
        "a write for a disconnected appliance fails closed and is never queued: {outcome:?}"
    );
}
