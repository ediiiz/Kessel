//! Shared fake transports for the hc-core tests; no real network.
//!
//! A [`FakeLink`] scripts a sequence of connection outcomes; each successful connect yields a
//! [`FakeSession`] whose steady-state behaviour (clean close, error, hang, or panic) is scripted
//! too. This is the in-process stand-in for `hc_transport::{aes_ws,tls_psk}::connect` +
//! `hc_transport::Session`.

#![allow(
    dead_code,
    unreachable_pub,
    reason = "each test binary compiles a subset of this shared harness"
)]

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use hc_core::bridge::{SessionWriter, guarded_session};
use hc_core::supervisor::{DeviceLink, DeviceSession, LinkError, UpdateSink};
use hc_core::write::{GuardedWriter, WriteCommand, WriteTransport};
use hc_proto::{
    Action, DeviceIdentity, Handshake, HandshakeConfig, IncomingMessage, OutgoingMessage,
    RandomNonce, ValueUpdate, Version,
};
use hc_transport::{
    Endpoints, FrameDecoder, FrameEncoder, FrameSink, FrameStream, Session, TransportError, WsFrame,
};
use parking_lot::Mutex;
use serde_json::Value;

/// A [`WriteTransport`] that records every command that reaches it.
#[derive(Clone, Default)]
pub struct RecordingTransport {
    /// Every command the guard let through to the transport.
    pub sent: Arc<Mutex<Vec<WriteCommand>>>,
}

impl RecordingTransport {
    /// A fresh recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many commands reached the transport.
    #[must_use]
    pub fn count(&self) -> usize {
        self.sent.lock().len()
    }
}

#[async_trait]
impl WriteTransport for RecordingTransport {
    async fn transmit(&mut self, command: &WriteCommand) -> Result<(), String> {
        self.sent.lock().push(command.clone());
        Ok(())
    }
}

// --- the real-`Sender` write path, with the bytes captured at the sink -------------------------
//
// These record bytes, not `WriteCommand`s: firmware parses JSON objects positionally, and a
// `serde_json::Value` comparison ignores key order. The sink writes into a `Vec`.

/// Every frame that reached the [`FrameSink`], as raw bytes.
#[derive(Clone, Default)]
pub struct RecordedWire {
    frames: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl RecordedWire {
    /// Every captured frame, in the order it was written.
    #[must_use]
    pub fn frames(&self) -> Vec<Vec<u8>> {
        self.frames.lock().clone()
    }

    /// Total bytes written; `0` means nothing left the process.
    #[must_use]
    pub fn byte_count(&self) -> usize {
        self.frames.lock().iter().map(Vec::len).sum()
    }

    /// The `msgID` of every captured frame, in send order.
    ///
    /// Parsed with `hc_proto::parse`; the frames are plaintext JSON here (see [`PlainEncoder`]).
    /// Panics on a frame without a `msgID`.
    #[must_use]
    pub fn msg_ids(&self) -> Vec<u64> {
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

/// A [`FrameSink`] that captures frames instead of writing them to a socket.
pub struct RecordingSink {
    wire: RecordedWire,
}

#[async_trait]
impl FrameSink for RecordingSink {
    async fn send_frame(&mut self, frame: WsFrame) -> Result<(), TransportError> {
        let bytes = match frame {
            WsFrame::Binary(bytes) => bytes,
            WsFrame::Text(text) => text.into_bytes(),
            // A control frame is not a write; refuse it rather than record it.
            other => return Err(TransportError::UnexpectedFrame(other.kind())),
        };
        self.wire.frames.lock().push(bytes);
        Ok(())
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        Ok(())
    }
}

/// A plaintext [`FrameEncoder`]: the message's own wire bytes, unsealed.
///
/// This is the TLS-PSK framing (no app-layer crypto, text frames), so the sink captures exactly
/// `OutgoingMessage::to_wire_bytes`. The AES transport would seal the same bytes.
pub struct PlainEncoder;

impl FrameEncoder for PlainEncoder {
    fn encode(&mut self, message: &OutgoingMessage) -> Result<WsFrame, TransportError> {
        Ok(WsFrame::Binary(message.to_wire_bytes()?))
    }
}

/// The receive half: yields `remaining` placeholder data frames, then reports the peer closed.
///
/// The bytes are meaningless: [`ScriptedDecoder`] ignores them and returns the scripted message.
pub struct ScriptedStream {
    remaining: usize,
}

#[async_trait]
impl FrameStream for ScriptedStream {
    async fn next_frame(&mut self) -> Result<WsFrame, TransportError> {
        if self.remaining == 0 {
            return Err(TransportError::Closed);
        }
        self.remaining -= 1;
        Ok(WsFrame::Binary(vec![0]))
    }
}

/// The decode half: hands back the scripted inbound messages in order, then reports closed.
pub struct ScriptedDecoder {
    messages: VecDeque<IncomingMessage>,
}

impl FrameDecoder for ScriptedDecoder {
    fn decode(&mut self, _frame: WsFrame) -> Result<IncomingMessage, TransportError> {
        self.messages.pop_front().ok_or(TransportError::Closed)
    }
}

/// A synthetic `POST /ei/initialValues`, the appliance's opening message.
///
/// `s_id` and `ed_msg_id` are the two values the FSM takes from it: per-connection counters, not
/// appliance identifiers.
#[must_use]
pub fn initial_values(s_id: u64, ed_msg_id: u64) -> IncomingMessage {
    IncomingMessage {
        s_id: Some(s_id),
        msg_id: Some(SERVER_MSG_ID),
        resource: Some("/ei/initialValues".to_owned()),
        version: Some(Version::Int(2)),
        action: Action::Post,
        data: Some(vec![serde_json::json!({"edMsgID": ed_msg_id})]),
        code: None,
    }
}

/// The `msgID` the synthetic appliance stamps on its opening `POST`. The FSM's RESPONSE reuses it
/// and spends no client id, so it must be distinguishable from any id the client allocator hands out.
pub const SERVER_MSG_ID: u64 = 900_000;

/// A synthetic `RESPONSE /ci/services` carrying a `ci`-version-2 service table — enough for the FSM
/// to build its request burst and so spend several client `msgID`s.
#[must_use]
pub fn services_response(s_id: u64) -> IncomingMessage {
    IncomingMessage {
        s_id: Some(s_id),
        msg_id: Some(SERVER_MSG_ID + 1),
        resource: Some("/ci/services".to_owned()),
        version: Some(Version::Int(1)),
        action: Action::Response,
        data: Some(vec![
            serde_json::json!({"service": "ci", "version": 2}),
            serde_json::json!({"service": "ro", "version": 1}),
            serde_json::json!({"service": "ei", "version": 2}),
        ]),
        code: None,
    }
}

/// A [`Handshake`] driven **outside any session** to the state a `sID` of `s_id` and an appliance
/// `edMsgID` of `ed_msg_id` leaves it in.
///
/// The emitted messages are dropped, so the counter is positioned as on a real connection but no
/// frame reaches any sink. Its `GET /ci/services` spent `ed_msg_id`, so the first id a writer
/// spends is `ed_msg_id + 1`.
#[must_use]
pub fn seeded_handshake(s_id: u64, ed_msg_id: u64) -> Handshake {
    let mut fsm = Handshake::new(HandshakeConfig {
        // Synthetic identity: no real `deviceName`/`deviceID`.
        identity: DeviceIdentity {
            name: "kessel-test".to_owned(),
            id: "00000000".to_owned(),
        },
    });
    let _dropped = fsm
        .on_message(initial_values(s_id, ed_msg_id), &mut RandomNonce)
        .expect("a well-formed POST /ei/initialValues advances the FSM");
    fsm
}

/// The session shape this harness builds: a recording sink, a plaintext encoder, and a scripted RX
/// half.
pub type RecordingSession = Session<RecordingSink, ScriptedStream, PlainEncoder, ScriptedDecoder>;

/// A session over the recording sink whose FSM is already positioned by `seeded_handshake`, with
/// nothing left to receive. The sink starts empty, so every byte it captures belongs to the write
/// path.
#[must_use]
pub fn seeded_session(wire: &RecordedWire, s_id: u64, ed_msg_id: u64) -> RecordingSession {
    Session::new(
        Endpoints::new(
            RecordingSink { wire: wire.clone() },
            ScriptedStream { remaining: 0 },
            PlainEncoder,
            ScriptedDecoder {
                messages: VecDeque::new(),
            },
        ),
        seeded_handshake(s_id, ed_msg_id),
        Box::new(RandomNonce),
    )
}

/// A session that drives its own handshake against a two-message script (`POST /ei/initialValues`
/// and `RESPONSE /ci/services`), spending the connection's first `msgID`s through the sink.
///
/// `Session::run_handshake` ends in `TransportError::Closed` once the script runs out; the frames
/// it emitted are already on the sink.
#[must_use]
pub fn handshaking_session(wire: &RecordedWire, s_id: u64, ed_msg_id: u64) -> RecordingSession {
    let mut messages = VecDeque::new();
    messages.push_back(initial_values(s_id, ed_msg_id));
    messages.push_back(services_response(s_id));
    Session::new(
        Endpoints::new(
            RecordingSink { wire: wire.clone() },
            ScriptedStream {
                remaining: messages.len(),
            },
            PlainEncoder,
            ScriptedDecoder { messages },
        ),
        Handshake::new(HandshakeConfig {
            identity: DeviceIdentity {
                name: "kessel-test".to_owned(),
                id: "00000000".to_owned(),
            },
        }),
        Box::new(RandomNonce),
    )
}

/// A session whose FSM is past the handshake, scripted to receive `notifies` in steady state.
///
/// The FSM is out of `AwaitingServices`, so a NOTIFY takes the same path as on a live connection.
/// The RX half yields `notifies.len()` frames and then reports the peer closed, so
/// `run_steady_state*` routes every scripted message and ends with `TransportError::Closed`.
#[must_use]
pub fn steady_state_session(
    wire: &RecordedWire,
    s_id: u64,
    ed_msg_id: u64,
    notifies: Vec<IncomingMessage>,
) -> RecordingSession {
    let mut fsm = seeded_handshake(s_id, ed_msg_id);
    let _burst = fsm
        .on_message(services_response(s_id), &mut RandomNonce)
        .expect("a well-formed RESPONSE /ci/services advances the FSM past the services step");
    let messages: VecDeque<IncomingMessage> = notifies.into();
    Session::new(
        Endpoints::new(
            RecordingSink { wire: wire.clone() },
            ScriptedStream {
                remaining: messages.len(),
            },
            PlainEncoder,
            ScriptedDecoder { messages },
        ),
        fsm,
        Box::new(RandomNonce),
    )
}

/// A synthetic `NOTIFY /ro/values` carrying one `{uid, value}` item — the steady-state shape.
#[must_use]
pub fn notify_values(s_id: u64, msg_id: u64, uid: u64, value: i64) -> IncomingMessage {
    IncomingMessage {
        s_id: Some(s_id),
        msg_id: Some(msg_id),
        resource: Some("/ro/values".to_owned()),
        version: Some(Version::Int(1)),
        action: Action::Notify,
        data: Some(vec![serde_json::json!({"uid": uid, "value": value})]),
        code: None,
    }
}

/// A [`GuardedWriter`] over a **real** `hc_transport::Sender`, plus the bytes it emits.
///
/// Built through `hc_core::bridge::guarded_session`, which takes `sID`, the `msgID` allocator and
/// the `ro` version from the session's own handshake, so this harness cannot seed an independent
/// counter.
///
/// The seeded handshake spent `ed_msg_id`, so the first frame this writer emits carries
/// `ed_msg_id + 1`.
#[must_use]
pub fn recording_write_path(
    s_id: u64,
    ed_msg_id: u64,
) -> (
    GuardedWriter<SessionWriter<RecordingSink, PlainEncoder>>,
    RecordedWire,
) {
    let wire = RecordedWire::default();
    let connection = seeded_session(&wire, s_id, ed_msg_id);
    let writer =
        guarded_session(connection).expect("a fresh session has not yet yielded its send handle");
    (writer, wire)
}

/// How a fake session behaves once connected.
#[derive(Clone)]
pub enum RunBehavior {
    /// Steady state ends cleanly at once (the device closes) — the loop reconnects.
    CloseCleanly,
    /// Steady state ends with an error.
    Fail(LinkError),
    /// Steady state delivers these updates, as a live NOTIFY does, and then the device closes.
    NotifyThenClose(Vec<ValueUpdate>),
    /// Steady state never returns — a wedged connection (drives the request-timeout path).
    Hang,
    /// Steady state panics — the isolation probe.
    Panic,
}

/// One scripted connection outcome.
#[derive(Clone)]
pub enum Attempt {
    /// The connect succeeds; the session seeds `initial` and then behaves per `behavior`.
    Connect {
        initial: Vec<ValueUpdate>,
        behavior: RunBehavior,
    },
    /// The connect fails (device offline).
    Fail(LinkError),
}

/// A fake connection.
pub struct FakeSession {
    initial: Vec<ValueUpdate>,
    behavior: RunBehavior,
    /// Incremented by [`DeviceSession::close`] — shared with the [`FakeLink`] that produced this
    /// session, so a test can assert the supervisor said goodbye to the appliance.
    closes: Arc<Mutex<u32>>,
}

#[async_trait]
impl DeviceSession for FakeSession {
    fn initial_state(&self) -> Vec<ValueUpdate> {
        self.initial.clone()
    }

    async fn run_steady_state(&mut self, sink: &mut dyn UpdateSink) -> Result<(), LinkError> {
        match &self.behavior {
            RunBehavior::CloseCleanly => Ok(()),
            RunBehavior::Fail(error) => Err(error.clone()),
            RunBehavior::NotifyThenClose(updates) => {
                // What a real session does: deliver what arrived, then report the close.
                sink.accept(updates);
                Ok(())
            }
            RunBehavior::Hang => std::future::pending().await,
            RunBehavior::Panic => panic!("fake session panic (isolation probe)"),
        }
    }

    async fn close(&mut self) {
        *self.closes.lock() += 1;
    }
}

/// A fake link. Replays `attempts` in order; once exhausted it repeats the last entry so a
/// `max_attempts`-bounded loop always has an outcome.
pub struct FakeLink {
    attempts: VecDeque<Attempt>,
    last: Attempt,
    /// How many times `connect` was called — for assertions.
    pub connect_calls: Arc<Mutex<u32>>,
    /// How many sessions this link produced were closed through [`DeviceSession::close`].
    pub closes: Arc<Mutex<u32>>,
}

impl FakeLink {
    /// A link that replays the given attempt script.
    #[must_use]
    pub fn new(attempts: Vec<Attempt>) -> Self {
        let last = attempts
            .last()
            .cloned()
            .unwrap_or(Attempt::Fail(LinkError::Offline("no script".to_owned())));
        Self {
            attempts: attempts.into(),
            last,
            connect_calls: Arc::new(Mutex::new(0)),
            closes: Arc::new(Mutex::new(0)),
        }
    }

    /// A link that always connects and closes cleanly, seeding `initial`.
    #[must_use]
    pub fn healthy(initial: Vec<ValueUpdate>) -> Self {
        Self::new(vec![Attempt::Connect {
            initial,
            behavior: RunBehavior::CloseCleanly,
        }])
    }

    /// A link that is always offline.
    #[must_use]
    pub fn always_offline() -> Self {
        Self::new(vec![Attempt::Fail(LinkError::Offline(
            "powered off".to_owned(),
        ))])
    }
}

#[async_trait]
impl DeviceLink for FakeLink {
    type Session = FakeSession;

    async fn connect(&mut self) -> Result<Self::Session, LinkError> {
        *self.connect_calls.lock() += 1;
        let attempt = self
            .attempts
            .pop_front()
            .unwrap_or_else(|| self.last.clone());
        match attempt {
            Attempt::Connect { initial, behavior } => Ok(FakeSession {
                initial,
                behavior,
                closes: Arc::clone(&self.closes),
            }),
            Attempt::Fail(error) => Err(error),
        }
    }
}

/// A convenience `ValueUpdate` for seeding tests: a value, revising no description.
#[must_use]
pub fn update(uid: u64, value: Value) -> ValueUpdate {
    ValueUpdate {
        uid,
        value,
        description: hc_proto::DescriptionUpdate::default(),
    }
}
