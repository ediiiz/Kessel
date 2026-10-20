//! The live device path: connect, handshake, and hold the connection open.
//!
//! Two things live here, and only one of them can lead to a write:
//!
//! 1. [`LiveConnectionTester`]: a read-only connection test. It connects, completes the handshake,
//!    reads the post-CONNECT `/ro` state and drops the session. It installs no writer.
//! 2. [`LiveDeviceLink`] / [`LiveDeviceSession`]: the long-lived device task. It does the same
//!    connect and handshake, seals the session's one send handle into a
//!    `hc_core::write::GuardedWriter`, installs it in [`crate::write::WriterRegistry`] for the life
//!    of the connection, and keeps the session alive in steady state.
//!
//! A writer is installed for every appliance, the cooktop and the oven included; nothing in this
//! module refuses one.
//!
//! A profile carrying an `iv` speaks the AES app-layer chain over `ws://host:80`; one without an
//! `iv` speaks TLS-PSK over `wss://host:443` with no app-layer crypto.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use hc_core::device::ConnectionState;
use hc_core::supervisor::{DeviceSession, LinkError, UpdateSink};
use hc_proto::{State, ValueUpdate};
use hc_transport::{FrameDecoder, FrameEncoder, FrameSink, FrameStream, HandshakeOutcome, Session};
use parking_lot::Mutex;

use crate::mqtt::MqttSettings;
use crate::onboarding::{ConnectionTarget, ConnectionTester};
use crate::write::WriterRegistry;

/// Which wire transport a profile's key material selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireTransport {
    /// `ws://host:80/homeconnect` with the `hc-crypto` AES chains (`aes-chain.md`).
    Aes,
    /// `wss://host:443/homeconnect` over TLS-PSK, plaintext framing (`tls-psk.md`).
    TlsPsk,
}

/// The transport `target` speaks, decided by the presence of an `iv`.
///
/// The `iv` is the discriminator rather than the manifest's `connectionType` because the AES path
/// cannot be built without one: a profile declaring `AES` but carrying no `iv` has no AES path.
#[must_use]
pub const fn select_transport(target: &ConnectionTarget) -> WireTransport {
    if target.iv64.is_some() {
        WireTransport::Aes
    } else {
        WireTransport::TlsPsk
    }
}

/// Why a live connection test failed.
///
/// Every variant's `Display` is safe to show: it becomes `ConnectionTest::Failed.reason`, which
/// reaches the SPA, the SSE inspector and the log. On the AES path the dialled host is the `haId`,
/// so [`Self::Transport`]'s detail is passed through [`redact_host`], and the key-material variant
/// carries a fixed class rather than the base64 error (which names an offending byte of the psk).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LiveTestError {
    /// The profile's `key`/`iv` cannot produce a usable session. **No socket is opened.**
    #[error("the profile's key material is unusable ({0}); no connection was attempted")]
    KeyMaterial(&'static str),
    /// A TLS-PSK appliance, in a binary built without the `tls-psk` feature. No socket is opened.
    #[error(
        "this build has no TLS-PSK transport and this appliance speaks TLS-PSK; no connection was \
         attempted"
    )]
    TlsPskUnavailable,
    /// The dial, the TLS handshake, the WebSocket upgrade or the protocol handshake failed.
    ///
    /// The payload is already host-redacted; see the type-level note.
    #[error("the connect or handshake failed: {0}")]
    Transport(String),
    /// The handshake ended somewhere other than `CONNECTED`.
    #[error("the handshake ended in state {0:?} rather than CONNECTED")]
    NotConnected(State),
}

/// What is reported instead of a non-ASCII error text, whose offsets cannot be spliced safely.
///
/// Names nothing: no host, no fragment of the original. See [`redact_host`].
const UNREDACTABLE_DETAIL: &str = "the connection failed, and the underlying error text could not \
                                   be host-redacted safely, so it is withheld";

/// Replaces every case-insensitive occurrence of `host` in `detail` with `<host>`.
///
/// On the AES path `host` is the `haId`, and `tokio-tungstenite`'s errors quote the URL they were
/// given. An empty `host` is returned unchanged rather than matching everywhere.
///
/// Non-ASCII input is replaced wholesale by `UNREDACTABLE_DETAIL`: the scan finds offsets in a
/// lowercased copy and splices them out of the original, which is sound only while lowercasing
/// preserves byte offsets (`İ` is two bytes and lowercases to three). Hosts and transport errors
/// are ASCII in practice.
#[must_use]
pub fn redact_host(detail: &str, host: &str) -> String {
    if host.is_empty() {
        return detail.to_owned();
    }
    if !detail.is_ascii() || !host.is_ascii() {
        return UNREDACTABLE_DETAIL.to_owned();
    }
    let needle = host.to_lowercase();
    let haystack = detail.to_lowercase();
    let mut out = String::with_capacity(detail.len());
    let mut cursor = 0_usize;
    while let Some(offset) = haystack.get(cursor..).and_then(|rest| rest.find(&needle)) {
        let at = cursor + offset;
        out.push_str(detail.get(cursor..at).unwrap_or_default());
        out.push_str("<host>");
        cursor = at + needle.len();
    }
    out.push_str(detail.get(cursor..).unwrap_or_default());
    out
}

/// What one probe observed. Carries no appliance body data.
#[derive(Debug, Clone)]
struct Probe {
    /// The handshake outcome: terminal state, `saw_403`, a notification **count**.
    outcome: HandshakeOutcome,
    /// Which TLS-PSK client identity connected, on the TLS path. `None` on the AES path.
    psk_identity: Option<String>,
}

/// The production [`ConnectionTester`]: a **real, read-only** connect + handshake.
///
/// Not installed by [`crate::state::AppState::new`], which uses
/// [`crate::onboarding::NoLiveTransport`] so no test binary can contact an appliance by default.
/// `main.rs` installs this one.
#[derive(Debug, Default)]
pub struct LiveConnectionTester {
    /// The most recent handshake's outcome, kept because `ConnectionTester::test` returns
    /// `Result<(), String>` and so cannot report `saw_403`.
    last: Mutex<Option<Probe>>,
}

impl LiveConnectionTester {
    /// A tester with no recorded outcome yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The most recent completed handshake's outcome, if one has completed.
    #[must_use]
    pub fn last_outcome(&self) -> Option<HandshakeOutcome> {
        self.last.lock().as_ref().map(|probe| probe.outcome)
    }

    /// Which TLS-PSK client identity the most recent TLS connect used — `""` or `"HCCOM_Local_App"`.
    ///
    /// `None` on the AES path, or before any TLS connect. Not a secret: both values are published
    /// constants.
    #[must_use]
    pub fn last_psk_identity(&self) -> Option<String> {
        self.last
            .lock()
            .as_ref()
            .and_then(|probe| probe.psk_identity.clone())
    }

    /// Performs one **read-only** connect + handshake against `target` and reports the outcome.
    ///
    /// Connect, handshake, read the post-CONNECT `/ro` state, disconnect. Nothing is written: the
    /// only method called on the session is `run_handshake`, and the session is dropped — closing
    /// the socket — before this returns.
    ///
    /// # Errors
    ///
    /// [`LiveTestError`]. The key-material and missing-transport variants are decided before any
    /// socket is opened.
    pub async fn probe(
        &self,
        target: &ConnectionTarget,
    ) -> Result<HandshakeOutcome, LiveTestError> {
        let probe = match select_transport(target) {
            WireTransport::Aes => aes_probe(target).await?,
            WireTransport::TlsPsk => tls_psk_probe(target).await?,
        };
        let outcome = probe.outcome;
        *self.last.lock() = Some(probe);
        if outcome.state == State::Connected {
            Ok(outcome)
        } else {
            Err(LiveTestError::NotConnected(outcome.state))
        }
    }
}

#[async_trait]
impl ConnectionTester for LiveConnectionTester {
    async fn test(&self, target: &ConnectionTarget) -> Result<(), String> {
        match self.probe(target).await {
            Ok(outcome) => {
                // A tolerated 403 does not fail the test: the FSM makes 403 fatal on
                // `/ci/services`, `/ci/authentication` and `/ro/*`, so reaching CONNECTED rules
                // those out. It is still logged rather than swallowed.
                if outcome.saw_403 {
                    tracing::warn!(
                        "the handshake completed but a tolerated response carried code 403; the \
                         connection is usable"
                    );
                }
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

/// The TLS-PSK client identities to try, in order.
///
/// `""` works on the appliances seen so far. The second is the identity hint TLS appliances
/// advertise; it is a defensive fallback for firmware that does check the client identity.
pub const PSK_IDENTITY_LADDER: [&str; 2] = ["", "HCCOM_Local_App"];

/// The app-layer identity the handshake FSM answers `/ei/initialValues` with.
///
/// hcpy's published client constants: fixed, never per-connection random.
fn app_identity() -> hc_proto::DeviceIdentity {
    hc_proto::DeviceIdentity {
        name: "hcpy".to_owned(),
        id: "0badcafe".to_owned(),
    }
}

/// The operator-facing class of a key-material rejection, with no key bytes in it.
///
/// `hc_crypto::CryptoError::Base64`'s own `Display` embeds the `base64` crate's message, which names
/// the offending **byte of the psk**. One byte is still key material, so the class is reported.
const fn key_material_class(error: &hc_crypto::CryptoError) -> &'static str {
    match error {
        hc_crypto::CryptoError::Base64(_) => "the `key` or `iv` is not valid URL-safe base64",
        hc_crypto::CryptoError::PskLength { .. } => "the `key` does not decode to 32 bytes",
        hc_crypto::CryptoError::IvLength { .. } => "the `iv` does not decode to 16 bytes",
        _ => "the key material was rejected",
    }
}

/// Renders a [`hc_transport::TransportError`] as a host-redacted reason.
fn transport_failure(error: &hc_transport::TransportError, host: &str) -> LiveTestError {
    LiveTestError::Transport(redact_host(&error.to_string(), host))
}

/// One read-only AES connect + handshake over `ws://host:80`.
async fn aes_probe(target: &ConnectionTarget) -> Result<Probe, LiveTestError> {
    use rand::SeedableRng as _;

    // Unreachable in practice: `select_transport` only chose AES because it saw an `iv`.
    let iv64 = target
        .iv64
        .as_ref()
        .ok_or(LiveTestError::KeyMaterial("the profile carries no `iv`"))?;

    // Decided before anything is dialled: unusable keys fail closed.
    let keys =
        hc_crypto::SessionKeys::from_base64(target.psk64.expose_secret(), iv64.expose_secret())
            .map_err(|error| LiveTestError::KeyMaterial(key_material_class(&error)))?;

    // Pad randomness from an OS-seeded CSPRNG.
    let pad = rand::rngs::StdRng::from_os_rng();
    let mut session =
        hc_transport::aes_ws::connect(&target.host, target.port, keys, app_identity(), pad)
            .await
            .map_err(|error| transport_failure(&error, &target.host))?;

    // Read-only: `run_handshake` drives the handshake and the post-CONNECT `/ro` reads and nothing
    // else. Dropping `session` on return closes the socket.
    let outcome = session
        .run_handshake()
        .await
        .map_err(|error| transport_failure(&error, &target.host))?;
    Ok(Probe {
        outcome,
        psk_identity: None,
    })
}

/// One read-only TLS-PSK connect + handshake over `wss://host:443`.
///
/// Walks [`PSK_IDENTITY_LADDER`], records the identity that connected, and keeps every failed
/// rung's error in the final reason.
#[cfg(feature = "tls-psk")]
async fn tls_psk_probe(target: &ConnectionTarget) -> Result<Probe, LiveTestError> {
    use hc_transport::tls_psk::{self, PskConfig};

    let psk = hc_crypto::decode_key(target.psk64.expose_secret())
        .map_err(|_ignored| LiveTestError::KeyMaterial("the `key` is not valid base64"))?;

    // Every rung's error is kept, so a total failure says which identities were tried and how each
    // one failed.
    let mut attempts: Vec<String> = Vec::new();
    for identity in PSK_IDENTITY_LADDER {
        let config = PskConfig::new(
            identity.as_bytes().to_vec(),
            hc_crypto::Secret::new(psk.clone()),
        );
        match tls_psk::connect(
            &target.host,
            target.port.unwrap_or(tls_psk::DEFAULT_TLS_PORT),
            &config,
            app_identity(),
        )
        .await
        {
            Ok(mut session) => {
                // Read-only, as on the AES path.
                let outcome = session
                    .run_handshake()
                    .await
                    .map_err(|error| transport_failure(&error, &target.host))?;
                return Ok(Probe {
                    outcome,
                    psk_identity: Some(identity.to_owned()),
                });
            }
            Err(error) => {
                let rendered = redact_host(&error.to_string(), &target.host);
                let label = if identity.is_empty() {
                    "identity \"\""
                } else {
                    "identity \"HCCOM_Local_App\""
                };
                attempts.push(format!("{label}: {rendered}"));
            }
        }
    }
    Err(LiveTestError::Transport(attempts.join("; then ")))
}

/// Without the `tls-psk` feature there is no TLS transport in this binary, so this fails closed.
#[cfg(not(feature = "tls-psk"))]
async fn tls_psk_probe(_target: &ConnectionTarget) -> Result<Probe, LiveTestError> {
    Err(LiveTestError::TlsPskUnavailable)
}

// ---------------------------------------------------------------------------------------------
// The long-lived device task. Unlike the connection test above, this keeps the connection open and
// therefore lets a write reach the appliance.
// ---------------------------------------------------------------------------------------------

/// The two things a device task does with a live session, behind a one-parameter trait.
///
/// `hc_transport::Session` is generic over its sink, stream, encoder and decoder, and the AES and
/// TLS-PSK transports instantiate all four differently. This trait keeps those types, including the
/// raw `FrameSink`, out of the server crate.
#[async_trait]
pub trait HeldSession: Send {
    /// Runs the heartbeat + NOTIFY loop until the connection drops
    /// (`hc_transport::Session::run_steady_state`).
    ///
    /// # Errors
    ///
    /// The transport's own error, unredacted; the caller redacts it (see
    /// [`LiveDeviceSession::run_steady_state`]).
    async fn steady_state(&mut self) -> Result<(), hc_transport::TransportError>;

    /// Sends a WebSocket Close, ending the session deliberately rather than by dropping the socket.
    ///
    /// Infallible: the session is over either way.
    ///
    /// Not named `close` because `hc_transport::Session` has an inherent `close`, and a trait method
    /// of the same name invites accidental infinite recursion.
    async fn close_socket(&mut self);

    /// Runs the same loop, delivering every routed notification to `sink` as it arrives.
    ///
    /// This is the production path. With a sink the transport drains its notification buffer into
    /// it, which bounds a long-lived connection's memory, so [`Self::notify_stream`] stays empty;
    /// the sink-free [`Self::steady_state`] lets the buffer accumulate for a bounded read-back pump.
    ///
    /// # Errors
    ///
    /// The transport's own error, unredacted; see [`Self::steady_state`].
    async fn steady_state_with(
        &mut self,
        sink: &mut dyn hc_transport::NotificationSink,
    ) -> Result<(), hc_transport::TransportError>;

    /// The appliance's `/ro` value snapshot — its current values (`hc_proto::Handshake::ro_values`).
    fn ro_values(&self) -> &[hc_proto::IncomingMessage];

    /// The NOTIFY / uncorrelated messages this connection has collected so far.
    ///
    /// Not named `notifications`, which would clash with `Session::notifications` in the forwarding
    /// impl below.
    fn notify_stream(&self) -> &[hc_proto::IncomingMessage];
}

#[async_trait]
impl<Si, St, E, D> HeldSession for Session<Si, St, E, D>
where
    Si: FrameSink,
    St: FrameStream,
    E: FrameEncoder,
    D: FrameDecoder,
{
    async fn steady_state(&mut self) -> Result<(), hc_transport::TransportError> {
        self.run_steady_state().await
    }

    async fn close_socket(&mut self) {
        self.close().await;
    }

    async fn steady_state_with(
        &mut self,
        sink: &mut dyn hc_transport::NotificationSink,
    ) -> Result<(), hc_transport::TransportError> {
        self.run_steady_state_with(Some(sink)).await
    }

    fn notify_stream(&self) -> &[hc_proto::IncomingMessage] {
        self.notifications()
    }

    fn ro_values(&self) -> &[hc_proto::IncomingMessage] {
        Self::ro_values(self)
    }
}

/// A live connection, held open, with its device's guarded writer installed in the registry.
///
/// The session's one send handle is spent right after CONNECTED on
/// `hc_core::bridge::take_guarded_session_boxed`, so it is inside a `GuardedWriter` before anything
/// else could ask for it, and the take-once flag makes every later request return `None`. Nothing
/// under `apps/**` names a route to a bare handle or a raw sink
/// (`tests/no_bare_handle_in_production.rs`). This is discipline plus a runtime flag, not a
/// type-system guarantee.
///
/// The registry entry lives exactly as long as this session: installed at CONNECTED, removed in
/// [`Self::run_steady_state`] when the connection ends, and again in [`Drop`] for a panic or a
/// cancelled task. The removal makes a write to a dead connection fail closed instead of
/// transmitting into a closed socket.
pub struct LiveDeviceSession<S> {
    session: S,
    slug: String,
    writers: Arc<WriterRegistry>,
    /// The values this connection's NOTIFY stream carried, decoded once at CONNECTED.
    initial: Vec<ValueUpdate>,
    /// This appliance's declared uids, for decoding updates as they arrive (see [`DecodingSink`]).
    known: BTreeSet<u64>,
    /// This device's MQTT bridge channel, when one exists.
    events: Option<crate::mqtt::DeviceEvents>,
    /// The writes on this connection waiting for the appliance's answer.
    acks: hc_core::acks::WriteAcks,
    /// Where this connection's end is recorded for the dashboard — the mirror of `writers`, and with
    /// the same lifetime: installed at CONNECTED, taken back when the connection ends.
    devices: Arc<crate::state::DeviceRegistry>,
    /// This appliance's model and the trace bus, for publishing rendered values to SSE.
    model: Arc<hc_profile::EntityModel>,
    bus: Arc<hc_core::trace::TraceBus>,
    /// The host this session was dialled at, kept **only** so it can be redacted out of error text.
    ///
    /// On the AES path the host is the `haId` and `tokio-tungstenite`'s errors quote the URL, so an
    /// unredacted `LinkError` would put an appliance identifier on the trace bus. See
    /// [`redact_host`].
    host: String,
}

impl<S> LiveDeviceSession<S> {
    /// The device's slug: safe to log, unlike a `haId`, serial or MAC.
    #[must_use]
    pub fn slug(&self) -> &str {
        &self.slug
    }
}

impl<S: HeldSession> LiveDeviceSession<S> {
    /// The NOTIFY / uncorrelated messages this connection has collected so far.
    ///
    /// These messages carry appliance body data and must never be logged. Decode the values with
    /// [`updates_from_notifications`] and publish those instead.
    #[must_use]
    pub fn notify_stream(&self) -> &[hc_proto::IncomingMessage] {
        self.session.notify_stream()
    }

    /// The appliance's `/ro` value snapshot — its current values as of the handshake.
    ///
    /// This is the only source of state for an appliance that pushes no NOTIFY, such as a standby
    /// dishwasher. Same logging rule as [`Self::notify_stream`].
    #[must_use]
    pub fn ro_values(&self) -> &[hc_proto::IncomingMessage] {
        self.session.ro_values()
    }

    /// Runs steady state for at most `budget`, so the NOTIFY stream can fill.
    ///
    /// Returns nothing: read [`Self::notify_stream`] afterwards and decode it with
    /// [`updates_from_notifications`], which needs the caller's set of known uids.
    ///
    /// Reading a value back after a write needs the connection pumped for a bounded time, while
    /// `run_steady_state` runs until the connection drops. The timeout cancels the steady-state
    /// future, not the session: the socket and the installed writer survive.
    pub async fn pump(&mut self, budget: core::time::Duration) {
        // A drop during the pump shows up as a short NOTIFY stream; the supervisor's loop handles a
        // real disconnection.
        let _elapsed_or_ended = tokio::time::timeout(budget, self.session.steady_state()).await;
    }
}

impl<S> core::fmt::Debug for LiveDeviceSession<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Only the slug is safe to render: `host` is the haId on the AES path, `initial` is
        // appliance body data, and `session` holds the chains.
        f.debug_struct("LiveDeviceSession")
            .field("slug", &self.slug)
            .finish_non_exhaustive()
    }
}

impl<S> Drop for LiveDeviceSession<S> {
    fn drop(&mut self) {
        // Backstop for the removal in `run_steady_state`; removing an absent key is a no-op.
        self.writers.remove(&self.slug);
        self.acks.close();
        // `run_device` drops the session before it dials again, so this never overwrites a newer
        // connection's `Connected`.
        self.devices.set_state(&self.slug, ConnectionState::Offline);
    }
}

#[async_trait]
impl<S: HeldSession> DeviceSession for LiveDeviceSession<S> {
    fn initial_state(&self) -> Vec<ValueUpdate> {
        self.initial.clone()
    }

    async fn run_steady_state(&mut self, sink: &mut dyn UpdateSink) -> Result<(), LinkError> {
        // The transport hands over raw notifications, `DecodingSink` decodes them against this
        // device's known uids, and the supervisor's sink reconciles them into its shadow.
        //
        // One call, not a loop of bounded slices: slicing would reset the heartbeat's state, so a
        // wedged socket would never be detected.
        let mut decoding = DecodingSink {
            known: &self.known,
            updates: sink,
            devices: &self.devices,
            slug: &self.slug,
            events: self.events.as_ref(),
            acks: &self.acks,
            sse: ValueEvents {
                slug: &self.slug,
                model: &self.model,
                bus: &self.bus,
            },
        };
        let ended = self.session.steady_state_with(&mut decoding).await;
        self.acks.close();
        // Offline in Home Assistant as soon as the socket is gone; the last will is only the backstop
        // for a daemon that dies.
        if let Some(events) = self.events.as_ref() {
            let _closed = events.send(crate::mqtt::DeviceEvent::Disconnected);
        }
        // No write may be routed to the ended connection; this runs before the error is rendered.
        self.writers.remove(&self.slug);
        self.devices.set_state(&self.slug, ConnectionState::Offline);
        let reason = match ended {
            Ok(()) => "the appliance closed the session".to_owned(),
            Err(error) => redact_host(&error.to_string(), &self.host),
        };
        // `LinkError::Ended` tells `hc_core::supervisor::run_device` to back off and redial; a clean
        // close is still an end.
        Err(LinkError::Ended(reason))
    }

    /// Sends the WebSocket Close the supervisor asks for, down to the real socket.
    ///
    /// Overrides the trait's no-op default so the appliance is told the session has ended.
    async fn close(&mut self) {
        self.session.close_socket().await;
    }
}

/// The adapter between the transport's raw notifications and the supervisor's decoded updates.
///
/// `hc-transport` has no device model, so it hands over [`hc_proto::IncomingMessage`]s; decoding them
/// into [`ValueUpdate`]s needs this appliance's declared uids, so the adapter is per-session.
///
/// `accept` runs between two reads of the socket, so anything slow here stalls the connection and
/// its heartbeat. Decoding a batch and taking the shadow's lock is the budget; a broker publish is
/// not.
///
/// Everything passing through carries appliance body data, so this type has no `Debug` and logs no
/// values.
struct DecodingSink<'a> {
    known: &'a BTreeSet<u64>,
    updates: &'a mut dyn UpdateSink,
    /// The registry, so a description revision reaches the mapping BOTH surfaces read.
    devices: &'a crate::state::DeviceRegistry,
    /// This device's registry key, and the device component of every topic a rebuild produces.
    slug: &'a str,
    /// The MQTT bridge's channel, when there is one.
    events: Option<&'a crate::mqtt::DeviceEvents>,
    /// Writes waiting for the appliance's answer.
    acks: &'a hc_core::acks::WriteAcks,
    /// Everything needed to put a rendered value on the SSE stream.
    sse: ValueEvents<'a>,
}

/// Publishes each decoded value onto the trace bus, as the SSE stream's `value` event.
///
/// The bus is the SSE stream's source and retains a ring buffer, so a dashboard opened after a device
/// connected still sees recent values. `TraceBus::emit` never awaits or blocks on a slow subscriber,
/// which is what makes it safe inside `UpdateSink::accept`.
///
/// The rendered value goes on the stream deliberately; appliance identifiers never do. The device
/// field is the slug and the uid is a number (`tests/l10_sse_sweep.rs`).
struct ValueEvents<'a> {
    slug: &'a str,
    model: &'a hc_profile::EntityModel,
    bus: &'a hc_core::trace::TraceBus,
}

impl ValueEvents<'_> {
    /// Emits one bus event per update, with the value rendered exactly as MQTT and REST render it.
    fn publish(&self, updates: &[ValueUpdate]) {
        for update in updates {
            let Ok(uid) = u16::try_from(update.uid) else {
                continue;
            };
            let rendered = hc_core::rendered_value(self.model, uid, &update.value);
            self.bus.emit(
                hc_core::trace::TraceRecord::new(
                    hc_core::trace::TraceLevel::Debug,
                    VALUE_TARGET,
                    "feature value",
                )
                .with_field(crate::types::VALUE_DEVICE_FIELD, self.slug)
                .with_field(crate::types::VALUE_UID_FIELD, uid.to_string())
                .with_field(crate::types::VALUE_FIELD, rendered),
            );
        }
    }
}

/// The trace target value events carry. Distinct from the supervisor's so a reader can filter them.
const VALUE_TARGET: &str = "kessel::values";

impl hc_transport::NotificationSink for DecodingSink<'_> {
    fn accept(&mut self, notifications: &[hc_proto::IncomingMessage]) {
        for message in notifications {
            let _answered = self.acks.resolve(message);
        }
        let updates = updates_from_notifications(notifications, self.known);
        // An empty batch is normal (e.g. `NOTIFY /ci/registeredDevices`); skip the shadow's lock.
        if updates.is_empty() {
            return;
        }
        // The shadow first: it is the authoritative store, and a failed bridge send must not lose a
        // value.
        self.updates.accept(&updates);
        // Then SSE, before MQTT, so a value the broker never takes is still visible in the inspector.
        self.sse.publish(&updates);
        // A `NOTIFY /ro/descriptionChange` revises a feature's access, availability or bounds; the
        // profile archive is only a snapshot from onboarding. Absorbed before the values are handed
        // on, so a bridge that re-announces reads a mapping that already has the revision.
        let redescribed = self
            .devices
            .update(self.slug, |entry| {
                entry.absorb_descriptions(self.slug, &updates)
            })
            .unwrap_or(false);
        if redescribed {
            // No value or bound is logged.
            tracing::info!(
                device = self.slug,
                "the appliance revised its own description; re-announcing"
            );
        }
        if let Some(events) = self.events {
            // Non-blocking, as `UpdateSink` requires. A closed channel means the bridge task is gone;
            // the value is already in the shadow.
            if redescribed {
                // Before the values, so Home Assistant has the revised config before a state only it
                // would accept.
                let _closed = events.send(crate::mqtt::DeviceEvent::Redescribed);
            }
            let _closed = events.send(crate::mqtt::DeviceEvent::Updates(updates));
        }
    }
}

/// The uids a device model declares — `hc_proto::apply_updates`'s "known" set.
///
/// An update for a uid the model does not declare is dropped rather than guessed.
#[must_use]
pub fn known_uids(model: &hc_profile::EntityModel) -> BTreeSet<u64> {
    model.entities().keys().map(|uid| u64::from(*uid)).collect()
}

/// Decodes the value updates a connection's NOTIFY stream carried.
///
/// Also used on the `/ro` snapshot, which has the same message shape.
#[must_use]
pub fn updates_from_notifications(
    notifications: &[hc_proto::IncomingMessage],
    known: &BTreeSet<u64>,
) -> Vec<ValueUpdate> {
    notifications
        .iter()
        .filter_map(|message| message.data.as_deref())
        .flat_map(|items| hc_proto::apply_updates(known, items))
        .collect()
}

/// Everything one device task needs to dial its appliance and register its writer.
///
/// `role` comes from the onboarded [`crate::state::DeviceEntry`] and travels with the writer.
/// Nothing in this module branches on it; write decisions are made only by the guard.
pub struct DeviceTaskConfig {
    /// The device slug: the registry key and the API path segment.
    pub slug: String,
    /// The appliance role class, passed to the sealer and thence to the guard.
    pub role: hc_core::write::DeviceRole,
    /// Where and how to dial, from the profile.
    pub target: ConnectionTarget,
    /// The uids the device model declares — see [`known_uids`].
    pub known_uids: BTreeSet<u64>,
    /// Where this connection's guarded writer is installed once CONNECTED.
    pub writers: Arc<WriterRegistry>,
    /// Where this device's shadow is registered, so its values are readable outside the task.
    pub shadows: Arc<crate::state::ShadowRegistry>,
    /// Where this device's lifecycle and values are reported to its MQTT bridge. `None` when no
    /// broker is configured.
    pub events: Option<crate::mqtt::DeviceEvents>,
    /// This appliance's model, for rendering values. `Arc` because a description can be large.
    pub model: Arc<hc_profile::EntityModel>,
    /// The trace bus, so a value update reaches SSE subscribers.
    pub bus: Arc<hc_core::trace::TraceBus>,
    /// The device registry, so this task's lifecycle transitions reach the dashboard.
    ///
    /// The task is the only thing that knows whether this appliance's socket is up; the supervisor's
    /// own trace events name the role class, not the slug.
    pub devices: Arc<crate::state::DeviceRegistry>,
}

impl core::fmt::Debug for DeviceTaskConfig {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Never render `target`: it holds key material, and its `host` is the haId on the AES path.
        f.debug_struct("DeviceTaskConfig")
            .field("slug", &self.slug)
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

/// One device's connect-and-hold link — `hc_core::supervisor::DeviceLink` for a real appliance.
///
/// Constructing one opens no socket; `connect` does, called by `hc_core::supervisor::run_device`.
/// The transport is chosen by [`select_transport`], and everything after the handshake is shared.
#[derive(Debug)]
pub struct LiveDeviceLink {
    config: DeviceTaskConfig,
}

impl LiveDeviceLink {
    /// A link that has not dialled. No socket is opened here.
    #[must_use]
    pub const fn new(config: DeviceTaskConfig) -> Self {
        Self { config }
    }

    /// Seals the session's one send handle into a `GuardedWriter` and installs it for this device.
    ///
    /// `hc_core::bridge::take_guarded_session_acked` takes the session's one handle and hands it
    /// straight to a `GuardedWriter`, so the handle never surfaces here. The writer is installed for
    /// every connected device.
    ///
    /// # Errors
    ///
    /// [`LinkError::Ended`] if the session has somehow already yielded its send handle.
    fn install_writer<Si, St, E, D>(
        &self,
        session: &mut Session<Si, St, E, D>,
    ) -> Result<hc_core::acks::WriteAcks, LinkError>
    where
        Si: FrameSink + Send + 'static,
        St: FrameStream,
        E: FrameEncoder + Send + 'static,
        D: FrameDecoder,
    {
        let acks = hc_core::acks::WriteAcks::new();
        let writer = hc_core::bridge::take_guarded_session_acked(session, acks.clone())
            .ok_or_else(|| {
                LinkError::Ended(
                    "the session had already yielded its one send handle, so no writer could be \
                     installed; the connection is dropped rather than run without a write path"
                        .to_owned(),
                )
            })?;
        self.config
            .writers
            .install(self.config.slug.clone(), writer);
        tracing::info!(
            device = %self.config.slug,
            role = %self.config.role.label(),
            "installed the device's guarded writer for this connection"
        );
        Ok(acks)
    }
}

/// Delegation, so one link type can produce **either** transport's session.
///
/// `hc_core::supervisor::DeviceLink` has a single associated `Session` type, and the AES and TLS-PSK
/// sessions differ, so they are erased behind [`HeldSession`]. None of its methods can send a frame.
#[async_trait]
impl HeldSession for Box<dyn HeldSession> {
    async fn steady_state_with(
        &mut self,
        sink: &mut dyn hc_transport::NotificationSink,
    ) -> Result<(), hc_transport::TransportError> {
        (**self).steady_state_with(sink).await
    }

    async fn steady_state(&mut self) -> Result<(), hc_transport::TransportError> {
        (**self).steady_state().await
    }

    async fn close_socket(&mut self) {
        (**self).close_socket().await;
    }

    fn notify_stream(&self) -> &[hc_proto::IncomingMessage] {
        (**self).notify_stream()
    }

    fn ro_values(&self) -> &[hc_proto::IncomingMessage] {
        (**self).ro_values()
    }
}

#[async_trait]
impl hc_core::supervisor::DeviceLink for LiveDeviceLink {
    type Session = LiveDeviceSession<Box<dyn HeldSession>>;

    async fn connect(&mut self) -> Result<Self::Session, LinkError> {
        // Connect + handshake, seal the writer, then hand back the still-open session: the handle is
        // spent on the guard before this returns, so nothing downstream can ask for one.
        //
        // The registry records Connecting here, Offline on failure, and Connected in `hold`.
        self.config
            .devices
            .set_state(&self.config.slug, ConnectionState::Connecting);
        let outcome = match select_transport(&self.config.target) {
            WireTransport::Aes => self.connect_aes().await,
            WireTransport::TlsPsk => self.connect_tls_psk().await,
        };
        if let Err(error) = outcome.as_ref() {
            // Safe to put on `GET /api/devices`: both `LinkError` arms are built from host-redacted
            // strings.
            self.config
                .devices
                .set_offline(&self.config.slug, &error.to_string());
        }
        outcome
    }
}

/// Turns a [`LiveTestError`] into the supervisor's [`LinkError::Offline`].
///
/// `Offline` rather than `Ended`: a powered-off appliance is normal, and `run_device` backs off and
/// retries. `LiveTestError`'s `Display` is already host-redacted.
fn offline(error: &LiveTestError) -> LinkError {
    LinkError::Offline(error.to_string())
}

impl LiveDeviceLink {
    /// Assembles the held session once a handshake has reached CONNECTED and the writer is installed.
    fn hold<S: HeldSession + 'static>(
        &self,
        session: S,
        acks: hc_core::acks::WriteAcks,
    ) -> LiveDeviceSession<Box<dyn HeldSession>> {
        // The /ro snapshot first, then the NOTIFY stream, so a later NOTIFY wins for the same uid.
        let mut initial = updates_from_notifications(session.ro_values(), &self.config.known_uids);
        initial.extend(updates_from_notifications(
            session.notify_stream(),
            &self.config.known_uids,
        ));
        tracing::info!(
            device = %self.config.slug,
            updates = initial.len(),
            snapshot = session.ro_values().len(),
            notifications = session.notify_stream().len(),
            "connection is live; seeding the shadow from the /ro snapshot plus the NOTIFY stream"
        );
        // The snapshot's description changes go into the mapping before anything is announced, so
        // the first discovery already has the appliance's current bounds and access.
        let _redescribed = self.config.devices.update(&self.config.slug, |entry| {
            entry.absorb_descriptions(&self.config.slug, &initial)
        });
        // `Connected` first: it makes the bridge publish discovery and subscribe to `…/set`, so the
        // values that follow have topics. The channel preserves the order.
        if let Some(events) = self.config.events.as_ref() {
            let _closed = events.send(crate::mqtt::DeviceEvent::Connected);
            if !initial.is_empty() {
                let _closed = events.send(crate::mqtt::DeviceEvent::Updates(initial.clone()));
            }
        }
        // Recorded here because this is the first point that knows which slug reached CONNECTED.
        self.config
            .devices
            .set_state(&self.config.slug, ConnectionState::Connected);
        LiveDeviceSession {
            session: Box::new(session) as Box<dyn HeldSession>,
            slug: self.config.slug.clone(),
            writers: Arc::clone(&self.config.writers),
            initial,
            known: self.config.known_uids.clone(),
            events: self.config.events.clone(),
            acks,
            devices: Arc::clone(&self.config.devices),
            model: Arc::clone(&self.config.model),
            bus: Arc::clone(&self.config.bus),
            host: self.config.target.host.clone(),
        }
    }

    /// One AES connect + handshake, held open.
    async fn connect_aes(&self) -> Result<LiveDeviceSession<Box<dyn HeldSession>>, LinkError> {
        use rand::SeedableRng as _;

        let iv64 =
            self.config.target.iv64.as_ref().ok_or_else(|| {
                offline(&LiveTestError::KeyMaterial("the profile carries no `iv`"))
            })?;
        // Decided before anything is dialled: unusable keys fail closed.
        let keys = hc_crypto::SessionKeys::from_base64(
            self.config.target.psk64.expose_secret(),
            iv64.expose_secret(),
        )
        .map_err(|error| offline(&LiveTestError::KeyMaterial(key_material_class(&error))))?;

        // Pad randomness from an OS-seeded CSPRNG.
        let pad = rand::rngs::StdRng::from_os_rng();
        let mut session = hc_transport::aes_ws::connect(
            &self.config.target.host,
            self.config.target.port,
            keys,
            app_identity(),
            pad,
        )
        .await
        .map_err(|error| offline(&transport_failure(&error, &self.config.target.host)))?;

        let outcome = session
            .run_handshake()
            .await
            .map_err(|error| offline(&transport_failure(&error, &self.config.target.host)))?;
        self.require_connected(&outcome)?;
        let acks = self.install_writer(&mut session)?;
        Ok(self.hold(session, acks))
    }

    /// A handshake that did not reach CONNECTED is not a connection.
    ///
    /// # Errors
    ///
    /// [`LinkError::Offline`] — retryable, because a device that answered but did not complete is the
    /// same operational situation as one that did not answer.
    fn require_connected(&self, outcome: &HandshakeOutcome) -> Result<(), LinkError> {
        if outcome.saw_403 {
            // A tolerated 403 does not fail the handshake (the FSM makes it fatal where it matters),
            // but it is never silent.
            tracing::warn!(
                device = %self.config.slug,
                "the handshake completed but a tolerated response carried code 403"
            );
        }
        if outcome.state == State::Connected {
            Ok(())
        } else {
            Err(offline(&LiveTestError::NotConnected(outcome.state)))
        }
    }
}

#[cfg(feature = "tls-psk")]
impl LiveDeviceLink {
    /// One TLS-PSK connect + handshake, held open.
    ///
    /// Walks [`PSK_IDENTITY_LADDER`] like the connection test and keeps every rung's error.
    async fn connect_tls_psk(&self) -> Result<LiveDeviceSession<Box<dyn HeldSession>>, LinkError> {
        let psk = hc_crypto::decode_key(self.config.target.psk64.expose_secret()).map_err(
            |_ignored| offline(&LiveTestError::KeyMaterial("the `key` is not valid base64")),
        )?;

        let mut attempts: Vec<String> = Vec::new();
        for identity in PSK_IDENTITY_LADDER {
            match self.try_psk_identity(identity, &psk).await {
                Ok(held) => return Ok(held),
                Err(rung) => attempts.push(rung),
            }
        }
        Err(LinkError::Offline(attempts.join("; then ")))
    }

    /// One rung of the identity ladder.
    ///
    /// # Errors
    ///
    /// The rung's host-redacted description, for the caller to accumulate.
    async fn try_psk_identity(
        &self,
        identity: &str,
        psk: &[u8],
    ) -> Result<LiveDeviceSession<Box<dyn HeldSession>>, String> {
        use hc_transport::tls_psk::{self, PskConfig};

        let label = if identity.is_empty() {
            "identity \"\""
        } else {
            "identity \"HCCOM_Local_App\""
        };
        let config = PskConfig::new(
            identity.as_bytes().to_vec(),
            hc_crypto::Secret::new(psk.to_vec()),
        );
        let mut session = tls_psk::connect(
            &self.config.target.host,
            self.config.target.port.unwrap_or(tls_psk::DEFAULT_TLS_PORT),
            &config,
            app_identity(),
        )
        .await
        .map_err(|error| {
            format!(
                "{label}: {}",
                redact_host(&error.to_string(), &self.config.target.host)
            )
        })?;

        let outcome = session.run_handshake().await.map_err(|error| {
            format!(
                "{label}: {}",
                transport_failure(&error, &self.config.target.host)
            )
        })?;
        self.require_connected(&outcome)
            .map_err(|error| format!("{label}: {error}"))?;
        let acks = self
            .install_writer(&mut session)
            .map_err(|error| format!("{label}: {error}"))?;
        Ok(self.hold(session, acks))
    }
}

/// Without the `tls-psk` feature there is no TLS transport in this binary, so a TLS appliance's device
/// task fails closed rather than dialling something else.
#[cfg(not(feature = "tls-psk"))]
impl LiveDeviceLink {
    async fn connect_tls_psk(&self) -> Result<LiveDeviceSession<Box<dyn HeldSession>>, LinkError> {
        Err(offline(&LiveTestError::TlsPskUnavailable))
    }
}

/// Spawns one supervised device task per device onboarded **at startup**, and returns how many.
///
/// A task is started, a socket opened and a writer installed for every onboarded device. Whether a
/// connected device may be written to is decided by `hc_core::write::check_access` from the
/// appliance's own declared `access`, not here.
///
/// Each device runs in its own task with its own `Shadow`, so one appliance's failure or panic
/// cannot touch another. Devices onboarded later are started by [`LiveDeviceLauncher`].
#[must_use]
pub fn spawn_device_tasks(state: &crate::state::AppState, mqtt: Option<&MqttSettings>) -> usize {
    let mut spawned = 0_usize;
    for summary in state.devices().summaries() {
        if spawn_device_task(state, mqtt, &summary.slug) {
            spawned = spawned.saturating_add(1);
        }
    }
    spawned
}

/// Spawns the supervised device task for **one** device, returning whether it was started.
///
/// `false` means `slug` is not in the registry. It does not check whether the device already has a
/// task; `crate::onboarding`'s duplicate guard does, because two concurrent sessions to one appliance
/// break both.
///
/// The `rng_seed` is derived from the slug so two appliances that drop together do not retry in
/// lockstep.
///
/// This opens a socket to a real appliance; `crate::onboarding::DeviceLauncher` keeps it out of test
/// binaries.
pub fn spawn_device_task(
    state: &crate::state::AppState,
    mqtt: Option<&MqttSettings>,
    slug: &str,
) -> bool {
    let Some(entry) = state.devices().entry(slug) else {
        return false;
    };
    let mut supervisor = hc_core::supervisor::Supervisor::new();
    // Created before the device task, so the first `Connected` has somewhere to go.
    let events = mqtt.map(|settings| {
        let (sender, stream) = crate::mqtt::device_events();
        crate::mqtt::spawn_device_bridge(state.clone(), settings.clone(), slug.to_owned(), stream);
        sender
    });
    let config = DeviceTaskConfig {
        slug: slug.to_owned(),
        role: entry.role.clone(),
        target: crate::onboarding::target_for(&entry.profile, entry.port),
        known_uids: known_uids(&entry.model),
        writers: Arc::clone(state.writers()),
        shadows: Arc::clone(state.shadows()),
        events,
        model: Arc::new(entry.model.clone()),
        bus: Arc::clone(state.bus()),
        devices: Arc::clone(state.devices()),
    };
    let device = hc_core::supervisor::DeviceConfig {
        role: entry.role.clone(),
        backoff: hc_core::backoff::Backoff::default(),
        rng_seed: slug_seed(slug),
        max_attempts: None,
        // The task is detached, so this receiver is the only way to close the session at shutdown.
        shutdown: Some(state.shutdown_signal()),
    };
    tracing::info!(
        device = %slug,
        role = %entry.role.label(),
        writable = entry.mapping.command_topics().next().is_some(),
        "starting a long-lived device task; its guarded writer is installed on CONNECT"
    );
    // One shadow per device, with a clone in the registry so values are readable outside the task.
    // Installed here rather than at CONNECTED because the task keeps the same `Arc` across
    // reconnects: an entry means "this device has a task"; connection liveness is
    // `WriterRegistry`'s to report.
    let shadow = Arc::new(Mutex::new(hc_core::shadow::Shadow::new()));
    state.shadows().install(slug, Arc::clone(&shadow));
    supervisor.spawn(
        LiveDeviceLink::new(config),
        device,
        Arc::clone(state.bus()),
        shadow,
    );

    // `run_device` loops forever, so there is nothing to join; dropping a `JoinHandle` detaches the
    // task rather than cancelling it.
    drop(supervisor.into_handles());
    true
}

/// The live [`crate::onboarding::DeviceLauncher`]: it starts a **real** device task, holding a **real**
/// connection, and installs that device's writer on CONNECT.
///
/// With this installed, a successful onboarding immediately holds the connection open and makes the
/// appliance writable from the API and MQTT. `main.rs` is the only place that installs it;
/// `AppState::new`'s default starts nothing.
#[derive(Debug, Clone)]
pub struct LiveDeviceLauncher {
    /// The broker the daemon resolved at startup, if one is configured, so every device uses the
    /// same settings.
    mqtt: Option<MqttSettings>,
}

impl LiveDeviceLauncher {
    /// Builds the launcher around the broker settings `main` resolved, if any.
    #[must_use]
    pub const fn new(mqtt: Option<MqttSettings>) -> Self {
        Self { mqtt }
    }
}

impl crate::onboarding::DeviceLauncher for LiveDeviceLauncher {
    fn launch(&self, state: &crate::state::AppState, slug: &str) -> bool {
        spawn_device_task(state, self.mqtt.as_ref(), slug)
    }
}

/// A per-device jitter seed derived from the slug.
///
/// FNV-1a: small, dependency-free and stable across runs. Used only to decorrelate reconnect timing
/// between devices.
fn slug_seed(slug: &str) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    slug.bytes().fold(OFFSET, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(PRIME)
    })
}

/// The device task's effect on the **registry's** connection state.
///
/// Contacts nothing: the session handed to `hold` is a local fake, or the test stops at a
/// key-material check decided before any dial. The profile is the scrubbed dishwasher fixture with
/// synthetic identifiers and keys.
#[cfg(test)]
mod tests {
    use super::*;

    use crate::state::{DeviceEntry, DeviceRegistry};

    const SLUG: &str = "dishwasher";

    /// The scrubbed dishwasher profile. Not from `secrets/`.
    fn profile() -> hc_profile::Profile {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/hc-profile/tests/fixtures/dishwasher-tls.zip");
        let bytes = std::fs::read(&path).expect("the scrubbed dishwasher fixture is readable");
        hc_profile::Profile::from_zip_bytes(&bytes).expect("the fixture parses")
    }

    /// A registry holding one dishwasher, in the state a restored device starts in.
    fn registry(profile: &hc_profile::Profile) -> Arc<DeviceRegistry> {
        let devices = Arc::new(DeviceRegistry::new());
        devices.insert(
            SLUG,
            DeviceEntry::from_profile(SLUG, hc_core::device::DeviceRole::Dishwasher, profile),
        );
        devices
    }

    /// A link for that dishwasher. Constructing one opens no socket.
    fn link(profile: &hc_profile::Profile, devices: &Arc<DeviceRegistry>) -> LiveDeviceLink {
        LiveDeviceLink::new(DeviceTaskConfig {
            slug: SLUG.to_owned(),
            role: hc_core::device::DeviceRole::Dishwasher,
            target: crate::onboarding::target_for(profile, None),
            known_uids: known_uids(profile.model()),
            writers: Arc::new(WriterRegistry::new()),
            shadows: Arc::new(crate::state::ShadowRegistry::new()),
            events: None,
            model: Arc::new(profile.model().clone()),
            bus: Arc::new(hc_core::trace::TraceBus::new(8)),
            devices: Arc::clone(devices),
        })
    }

    /// A session that never carries anything: no `/ro` snapshot, no NOTIFYs, no frames out.
    struct Idle;

    #[async_trait]
    impl HeldSession for Idle {
        async fn steady_state_with(
            &mut self,
            _sink: &mut dyn hc_transport::NotificationSink,
        ) -> Result<(), hc_transport::TransportError> {
            Ok(())
        }

        async fn steady_state(&mut self) -> Result<(), hc_transport::TransportError> {
            Ok(())
        }

        async fn close_socket(&mut self) {}

        fn notify_stream(&self) -> &[hc_proto::IncomingMessage] {
            &[]
        }

        fn ro_values(&self) -> &[hc_proto::IncomingMessage] {
            &[]
        }
    }

    /// A device restored at startup must be reported `Connected` once its task holds a session, not
    /// left at the onboarding test's verdict.
    #[test]
    fn a_live_connection_is_recorded_in_the_registry() {
        let profile = profile();
        let devices = registry(&profile);
        let link = link(&profile, &devices);

        // The CONNECTED point: `connect` calls this once the handshake has completed and the guarded
        // writer is installed.
        let held = link.hold(Idle, hc_core::acks::WriteAcks::new());

        assert_eq!(
            devices.entry(SLUG).expect("the device is registered").state,
            ConnectionState::Connected,
            "a device whose task is holding a live session must not be summarised as offline"
        );
        drop(held);
    }

    /// A `Connected` that is never taken back would show a working appliance for a socket that is
    /// gone. Both endings are covered: `run_steady_state` returning, and `Drop` for a cancelled or
    /// panicking task.
    #[tokio::test]
    async fn the_end_of_a_connection_records_offline() {
        let profile = profile();

        let devices = registry(&profile);
        let mut held = link(&profile, &devices).hold(Idle, hc_core::acks::WriteAcks::new());
        let mut sink = NoSink;
        let ended = held.run_steady_state(&mut sink).await;
        assert!(
            matches!(ended, Err(LinkError::Ended(_))),
            "the fake session ends immediately, which is a connection that ended"
        );
        assert_eq!(
            devices.entry(SLUG).expect("the device is registered").state,
            ConnectionState::Offline,
            "a device whose connection has ended must not still be summarised as connected"
        );

        let devices = registry(&profile);
        let held = link(&profile, &devices).hold(Idle, hc_core::acks::WriteAcks::new());
        drop(held);
        assert_eq!(
            devices.entry(SLUG).expect("the device is registered").state,
            ConnectionState::Offline,
            "and the backstop must cover a session dropped without running steady state"
        );
    }

    /// A dial that fails leaves the device offline rather than at whatever it last was.
    ///
    /// Contacts nothing: the key material is nonsense, so `connect_aes` fails at
    /// `SessionKeys::from_base64` before any socket is opened.
    #[tokio::test]
    async fn a_failed_dial_records_offline() {
        let profile = profile();
        let devices = registry(&profile);
        devices.set_state(SLUG, ConnectionState::Connected);

        let mut link = link(&profile, &devices);
        link.config.target.connection_type = hc_profile::ConnectionType::Aes;
        link.config.target.psk64 = hc_crypto::Secret::new("!!! not base64 !!!".to_owned());
        link.config.target.iv64 = Some(hc_crypto::Secret::new("!!! nor this !!!".to_owned()));

        let outcome = hc_core::supervisor::DeviceLink::connect(&mut link).await;
        assert!(
            matches!(outcome, Err(LinkError::Offline(_))),
            "unusable key material is a fail-closed offline verdict"
        );
        assert_eq!(
            devices.entry(SLUG).expect("the device is registered").state,
            ConnectionState::Offline,
            "a device that could not be dialled must not keep an earlier `Connected`"
        );
    }

    /// An update sink that accepts and forgets.
    struct NoSink;

    impl UpdateSink for NoSink {
        fn accept(&mut self, _updates: &[ValueUpdate]) {}
    }
}
