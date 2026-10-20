//! Shared fake-device harness for the `hc-transport` integration tests.
//!
//! The fake device is an in-process transport pair built on `tokio::sync::mpsc` channels: the client
//! end implements [`FrameSink`]/[`FrameStream`], the device end is driven by hand. The corpus
//! (`fixtures/aes/hob-session.json`) is embedded with `include_str!`.
//!
//! The device replays the recorded rx `wire_hex` bytes verbatim, in receive order. They form a
//! continuous ciphertext chain under the corpus's synthetic keys, so the client's fresh opener
//! decrypts them exactly.

#![allow(
    dead_code,
    unreachable_pub,
    reason = "each integration test binary compiles this module independently and uses a subset"
)]

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use hc_crypto::SessionKeys;
use hc_proto::{DeviceIdentity, Handshake, HandshakeConfig, RandomNonce};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt as _};
use tracing_subscriber::registry::Registry;

use hc_transport::TransportError;
use hc_transport::aes_ws::{AesDecoder, AesEncoder};
use hc_transport::session::{Endpoints, Session};
use hc_transport::transport::{FrameSink, FrameStream, WsFrame};

const CORPUS_JSON: &str = include_str!("../../../../fixtures/aes/hob-session.json");

#[derive(Debug, Deserialize)]
pub struct Corpus {
    pub key: CorpusKey,
    pub client: CorpusClient,
    pub frames: Vec<Frame>,
}

#[derive(Debug, Deserialize)]
pub struct CorpusKey {
    /// Synthetic and published; not appliance key material.
    pub psk64: String,
    pub iv64: String,
}

#[derive(Debug, Deserialize)]
pub struct CorpusClient {
    pub device_name: String,
    pub device_id: String,
}

#[derive(Debug, Deserialize)]
pub struct Frame {
    pub seq: u32,
    /// `"rx"` = appliance → client, `"tx"` = client → appliance.
    pub direction: String,
    pub resource: Option<String>,
    pub wire_hex: String,
    pub action: String,
    #[serde(default)]
    pub json: Value,
}

impl Frame {
    #[must_use]
    pub fn wire(&self) -> Vec<u8> {
        hex::decode(&self.wire_hex).expect("wire_hex is hex")
    }

    #[must_use]
    pub fn is_rx(&self) -> bool {
        self.direction == "rx"
    }
}

#[must_use]
pub fn corpus() -> Corpus {
    serde_json::from_str(CORPUS_JSON).expect("the golden corpus parses")
}

/// The raw embedded corpus text, so a redaction test can show a sentinel really is present in a
/// body before asserting `Debug` withholds it.
#[must_use]
pub const fn corpus_raw() -> &'static str {
    CORPUS_JSON
}

impl Corpus {
    #[must_use]
    pub fn keys(&self) -> SessionKeys {
        SessionKeys::from_base64(&self.key.psk64, &self.key.iv64)
            .expect("the synthetic psk64/iv64 decode and validate")
    }

    #[must_use]
    pub fn identity(&self) -> DeviceIdentity {
        DeviceIdentity {
            name: self.client.device_name.clone(),
            id: self.client.device_id.clone(),
        }
    }

    /// The rx frames' wire bytes, in receive order — what the device replays.
    #[must_use]
    pub fn rx_wire(&self) -> Vec<Vec<u8>> {
        self.frames
            .iter()
            .filter(|frame| frame.is_rx())
            .map(Frame::wire)
            .collect()
    }
}

// ---- the in-process transport pair -------------------------------------------------------------

/// The client's send half: writes frames into the channel the device reads.
pub struct FakeSink {
    to_device: UnboundedSender<WsFrame>,
}

/// The client's receive half: reads frames the device wrote.
pub struct FakeStream {
    from_device: UnboundedReceiver<WsFrame>,
}

/// The appliance end of the wire — the test drives this by hand.
pub struct FakeDevice {
    to_client: UnboundedSender<WsFrame>,
    from_client: UnboundedReceiver<WsFrame>,
}

/// Builds one in-process connection: `(client_sink, client_stream, device)`.
#[must_use]
pub fn fake_wire() -> (FakeSink, FakeStream, FakeDevice) {
    let (to_device, from_client) = unbounded_channel();
    let (to_client, from_device) = unbounded_channel();
    (
        FakeSink { to_device },
        FakeStream { from_device },
        FakeDevice {
            to_client,
            from_client,
        },
    )
}

#[async_trait]
impl FrameSink for FakeSink {
    async fn send_frame(&mut self, frame: WsFrame) -> Result<(), TransportError> {
        self.to_device
            .send(frame)
            .map_err(|_dropped| TransportError::Closed)
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        // Signal closure to the device, then let the sender drop with the struct.
        let _ = self.to_device.send(WsFrame::Close);
        Ok(())
    }
}

#[async_trait]
impl FrameStream for FakeStream {
    async fn next_frame(&mut self) -> Result<WsFrame, TransportError> {
        self.from_device.recv().await.ok_or(TransportError::Closed)
    }
}

impl FakeDevice {
    /// Sends one frame to the client.
    pub fn send(&self, frame: WsFrame) {
        let _ = self.to_client.send(frame);
    }

    /// Sends one binary data frame to the client.
    pub fn send_binary(&self, bytes: Vec<u8>) {
        self.send(WsFrame::Binary(bytes));
    }

    /// Receives the next frame the client sent, or `None` if the client's sink was dropped/closed.
    pub async fn recv(&mut self) -> Option<WsFrame> {
        self.from_client.recv().await
    }

    /// Receives the next frame within `ms`, or `None` on timeout / channel close — so a test asserts
    /// on "a close arrived" without ever hanging on the red path.
    pub async fn recv_within(&mut self, ms: u64) -> Option<WsFrame> {
        tokio::time::timeout(
            std::time::Duration::from_millis(ms),
            self.from_client.recv(),
        )
        .await
        .ok()
        .flatten()
    }

    /// Replays every rx frame's recorded wire bytes as binary frames, in receive order.
    pub fn replay_rx(&self, corpus: &Corpus) {
        for wire in corpus.rx_wire() {
            self.send_binary(wire);
        }
    }
}

// ---- session assembly for the tests ------------------------------------------------------------

/// The fake session type the tests drive. `StdRng` is a `Send` CSPRNG (`OsRng` is not directly
/// `RngCore` in rand 0.9, and a spawned concurrency test needs `Send`); byte-exactness of tx is not
/// needed because the device replays rx verbatim.
pub type FakeSession = Session<FakeSink, FakeStream, AesEncoder<rand::rngs::StdRng>, AesDecoder>;

/// A seeded, `Send` pad source for the fake sessions.
#[must_use]
pub fn test_pad() -> rand::rngs::StdRng {
    use rand::SeedableRng as _;
    rand::rngs::StdRng::seed_from_u64(0x_F0_5E_55_E1)
}

// ---- trace capture -----------------------------------------------------------------------------
//
// [`CaptureLayer`] mirrors `hc_core::trace::TraceBusLayer`'s field rendering (`record_str` verbatim,
// everything else through `Debug`, `message` split out), so a test sweeping the captured fields sees
// the same strings an SSE `TraceFrame.fields` would carry. hc-core cannot be a dev-dependency (it
// depends on hc-transport); `capture_renders_fields_like_the_trace_bus` pins the mirror.

/// One captured `tracing` event, rendered as `hc_core::trace::TraceEvent` would render it.
#[derive(Debug, Clone)]
pub struct CapturedEvent {
    /// The `tracing` target.
    pub target: String,
    /// The event's `message` field.
    pub message: String,
    /// Every other field, rendered to a string.
    pub fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    /// The rendered value of `key`, if the event carried it.
    #[must_use]
    pub fn field(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }

    /// Every `key=value` pair plus the target and message, as the strings a leak sweep must search.
    #[must_use]
    pub fn as_swept_strings(&self) -> Vec<String> {
        let mut out = vec![self.target.clone(), self.message.clone()];
        out.extend(
            self.fields
                .iter()
                .map(|(key, value)| format!("{key}={value}")),
        );
        out
    }
}

/// A shared, append-only capture of every event a `tracing` layer saw.
#[derive(Debug, Clone, Default)]
pub struct Captured {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl Captured {
    /// Every event captured so far, in emission order.
    #[must_use]
    pub fn events(&self) -> Vec<CapturedEvent> {
        self.events
            .lock()
            .expect("the capture mutex is never poisoned")
            .clone()
    }

    /// Only the events a trace consumer classifies as frames: those carrying a `direction` field.
    #[must_use]
    pub fn frames(&self) -> Vec<CapturedEvent> {
        self.events()
            .into_iter()
            .filter(|event| event.fields.contains_key("direction"))
            .collect()
    }
}

/// The capturing layer.
#[derive(Debug, Clone, Default)]
pub struct CaptureLayer {
    captured: Captured,
}

impl<S: Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = CaptureVisitor::default();
        event.record(&mut visitor);
        self.captured
            .events
            .lock()
            .expect("the capture mutex is never poisoned")
            .push(CapturedEvent {
                target: event.metadata().target().to_owned(),
                message: visitor.message,
                fields: visitor.fields,
            });
    }
}

/// Mirrors `hc_core::trace`'s `FieldVisitor` exactly: `record_str` verbatim, everything else through
/// `Debug`, with `message` split out from the rest.
#[derive(Default)]
struct CaptureVisitor {
    message: String,
    fields: BTreeMap<String, String>,
}

impl CaptureVisitor {
    fn store(&mut self, field: &Field, rendered: String) {
        if field.name() == "message" {
            self.message = rendered;
        } else {
            self.fields.insert(field.name().to_owned(), rendered);
        }
    }
}

impl Visit for CaptureVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.store(field, format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.store(field, value.to_owned());
    }
}

/// Installs a capturing subscriber as the **thread-local** default for as long as the returned
/// guard lives, and hands back the capture.
///
/// Thread-local because a test binary runs many tests in one process and a global subscriber can
/// only be installed once. A bare `Registry` enables every level, which a per-frame `trace!` needs.
#[must_use]
pub fn capture_traces() -> (Captured, tracing::subscriber::DefaultGuard) {
    let layer = CaptureLayer::default();
    let captured = layer.captured.clone();
    let guard = tracing::subscriber::set_default(Registry::default().with(layer));
    (captured, guard)
}

/// Assembles a client [`Session`] over the fake transport, in the client role.
#[must_use]
pub fn client_session(corpus: &Corpus, sink: FakeSink, stream: FakeStream) -> FakeSession {
    let keys = corpus.keys();
    let endpoints = Endpoints::new(
        sink,
        stream,
        AesEncoder::new(keys.clone(), test_pad()),
        AesDecoder::new(keys),
    );
    let fsm = Handshake::new(HandshakeConfig {
        identity: corpus.identity(),
    });
    Session::new(endpoints, fsm, Box::new(RandomNonce))
}
