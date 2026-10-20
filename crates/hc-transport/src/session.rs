//! The transport-agnostic session loop.
//!
//! A [`Session`] is generic over a [`FrameSink`]/[`FrameStream`] pair (the byte transport) and a
//! [`FrameEncoder`]/[`FrameDecoder`] pair (the message framing). It owns:
//!
//! - driving the [`hc_proto::Handshake`] FSM to CONNECTED;
//! - correlation and timeout: every `GET` is registered in the [`hc_proto::Correlator`], resolved
//!   by `msgID`, and fails after [`hc_proto::RESPONSE_TIMEOUT_MS`];
//! - consuming the post-CONNECT `/ro/allMandatoryValues` and `/ro/allDescriptionChanges` responses;
//! - the heartbeat: a WS ping every 20 s, failing on a missed pong;
//! - serialised sends ([`Sender`]): one lock spans encode and write on the TX side; the RX loop
//!   takes no part of it.

use std::sync::Arc;

use hc_proto::{Action, Correlator, Handshake, IncomingMessage, OutgoingMessage, State, Version};
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant, timeout};

use crate::codec::{FrameDecoder, FrameEncoder};
use crate::error::TransportError;
use crate::transport::{FrameSink, FrameStream, WsFrame};

/// The heartbeat ping interval and the pong deadline, in milliseconds.
///
/// A WebSocket-level ping every 20 s; a pong not received before the next ping fails the
/// connection. The value equals [`hc_proto::RESPONSE_TIMEOUT_MS`].
pub const HEARTBEAT_INTERVAL_MS: u64 = hc_proto::RESPONSE_TIMEOUT_MS;

/// The resource whose negotiated `version` [`Session::take_guarded`] resolves for the write path.
///
/// Used only to look up the version for the `ro` service; the `POST /ro/values` frame itself is
/// built in `hc-core`.
pub const WRITE_RESOURCE: &str = "/ro/values";

/// Where a session delivers the notifications it routes while steady state is running.
///
/// Receives raw [`IncomingMessage`]s; decoding them needs the device model, which lives above this
/// crate.
///
/// `accept` runs inside the steady-state loop and must not block: anything slow stalls the
/// connection and its heartbeat. The messages carry appliance body data and must never be printed.
pub trait NotificationSink: Send {
    /// Accepts the notifications drained by one steady-state read, in arrival order.
    fn accept(&mut self, notifications: &[IncomingMessage]);
}

/// The result of driving the handshake to CONNECTED and consuming the post-CONNECT `/ro` state.
///
/// Body-free: the terminal state, whether a `403` was seen, and a count of routed notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandshakeOutcome {
    /// The FSM's terminal state — [`State::Connected`] on success.
    pub state: State,
    /// Whether any handshake response carried `code == 403`.
    pub saw_403: bool,
    /// How many NOTIFY or uncorrelated messages were routed to the general handler during the
    /// handshake.
    pub notifications: usize,
}

/// The send half of a session: the sink and the encoder, together behind one lock.
///
/// One unit so the lock in [`Sender::send`] spans encode and write: the TX chain must advance in
/// exactly the order frames hit the wire.
struct Tx<Si, E> {
    sink: Si,
    encoder: E,
}

/// A cloneable handle for concurrent sends.
///
/// Every clone shares one lock, so concurrent sends serialise and each seals and writes before the
/// next begins. The RX loop never waits on it.
pub struct Sender<Si, E> {
    tx: Arc<Mutex<Tx<Si, E>>>,
}

impl<Si, E> core::fmt::Debug for Sender<Si, E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Sender").finish_non_exhaustive()
    }
}

impl<Si, E> Clone for Sender<Si, E> {
    fn clone(&self) -> Self {
        Self {
            tx: Arc::clone(&self.tx),
        }
    }
}

impl<Si: FrameSink, E: FrameEncoder> Sender<Si, E> {
    /// Encodes and writes one message, holding the TX lock across both.
    ///
    /// # Errors
    ///
    /// [`TransportError`] if the message cannot be encoded or written.
    pub async fn send(&self, message: &OutgoingMessage) -> Result<(), TransportError> {
        // Encode and write under one lock, or the TX chain and the wire order diverge and the peer
        // desyncs.
        let mut tx = self.tx.lock().await;
        let frame = tx.encoder.encode(message)?;
        tx.sink.send_frame(frame).await
    }
}

/// The four injected halves of a connection.
///
/// The fields are private so an `Endpoints` is one-way: the halves go in and come out only through
/// [`Session::new`]. This is not a wire-access boundary, since whoever constructs a [`FrameSink`]
/// can already write to it.
pub struct Endpoints<Si, St, E, D> {
    /// The send half of the byte transport.
    sink: Si,
    /// The receive half of the byte transport.
    stream: St,
    /// The message encoder (holds the TX chain, for AES).
    encoder: E,
    /// The message decoder (holds the RX chain, for AES).
    decoder: D,
}

impl<Si, St, E, D> Endpoints<Si, St, E, D> {
    /// Bundles the four halves of a connection for [`Session::new`].
    pub const fn new(sink: Si, stream: St, encoder: E, decoder: D) -> Self {
        Self {
            sink,
            stream,
            encoder,
            decoder,
        }
    }
}

impl<Si, St, E, D> core::fmt::Debug for Endpoints<Si, St, E, D> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Endpoints").finish_non_exhaustive()
    }
}

/// A live session over one connection.
pub struct Session<Si, St, E, D> {
    tx: Arc<Mutex<Tx<Si, E>>>,
    stream: St,
    decoder: D,
    fsm: Handshake,
    nonce: Box<dyn hc_proto::NonceSource + Send>,
    correlator: Correlator,
    start: Instant,
    notifications: usize,
    /// Whether the session's one send handle has already been handed out.
    handle_taken: bool,
}

impl<Si, St, E, D> core::fmt::Debug for Session<Si, St, E, D> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The FSM's `Debug` redacts its bodies; the transport halves, codecs and nonce source are
        // omitted.
        f.debug_struct("Session")
            .field("fsm", &self.fsm)
            .field("correlator", &self.correlator)
            .field("notifications", &self.notifications)
            .finish_non_exhaustive()
    }
}

impl<Si, St, E, D> Session<Si, St, E, D>
where
    Si: FrameSink,
    St: FrameStream,
    E: FrameEncoder,
    D: FrameDecoder,
{
    /// Assembles a session from its four injected halves plus the FSM and its nonce source.
    pub fn new(
        endpoints: Endpoints<Si, St, E, D>,
        fsm: Handshake,
        nonce: Box<dyn hc_proto::NonceSource + Send>,
    ) -> Self {
        let Endpoints {
            sink,
            stream,
            encoder,
            decoder,
        } = endpoints;
        Self {
            tx: Arc::new(Mutex::new(Tx { sink, encoder })),
            stream,
            decoder,
            fsm,
            nonce,
            correlator: Correlator::new(),
            start: Instant::now(),
            notifications: 0,
            handle_taken: false,
        }
    }

    /// A bare send handle, consuming the session.
    ///
    /// `None` if this session already yielded its handle: this shares the take-once flag with
    /// [`take_guarded`](Self::take_guarded) and [`into_guarded`](Self::into_guarded), so a session
    /// yields one handle in its lifetime. The handle can still be cloned for concurrent sends.
    ///
    /// Anything that writes to an appliance should use [`take_guarded`](Self::take_guarded), which
    /// also hands over the session's `msgID` allocator.
    #[must_use]
    pub fn into_sender(self) -> Option<Sender<Si, E>> {
        if self.handle_taken {
            return None;
        }
        // No need to set the flag: the session is consumed.
        Some(Sender {
            tx: Arc::clone(&self.tx),
        })
    }

    /// Hands this session's send handle and per-connection write state to `seal`, at most once.
    ///
    /// `None` after the first handle has been taken by this method,
    /// [`into_guarded`](Self::into_guarded) or [`into_sender`](Self::into_sender). It borrows so
    /// the session stays alive for [`run_steady_state`](Self::run_steady_state), and takes a closure
    /// so this crate does not need to name `hc-core`'s role type.
    ///
    /// `seal` receives:
    ///
    /// 1. the [`Sender`];
    /// 2. `sID` ([`hc_proto::Handshake::s_id`]), assigned by the appliance in its opening
    ///    `POST /ei/initialValues`;
    /// 3. a clone of the connection's [`hc_proto::MsgIdAllocator`] handle, so ids spent by the
    ///    writer are never reused by the session;
    /// 4. the negotiated `version` for [`WRITE_RESOURCE`].
    pub fn take_guarded<W>(
        &mut self,
        seal: impl FnOnce(Sender<Si, E>, u64, hc_proto::MsgIdAllocator, Version) -> W,
    ) -> Option<W> {
        if self.handle_taken {
            return None;
        }
        self.handle_taken = true;
        let sender = Sender {
            tx: Arc::clone(&self.tx),
        };
        Some(seal(
            sender,
            self.fsm.s_id(),
            self.fsm.msg_ids(),
            self.fsm.version_for(WRITE_RESOURCE),
        ))
    }

    /// [`take_guarded`](Self::take_guarded), consuming the session.
    ///
    /// Nothing survives but whatever `seal` returns. `None` if this session already yielded its
    /// handle. A writer that needs the session kept alive for
    /// [`run_steady_state`](Self::run_steady_state) uses `take_guarded` instead.
    #[must_use]
    pub fn into_guarded<W>(
        self,
        seal: impl FnOnce(Sender<Si, E>, u64, hc_proto::MsgIdAllocator, Version) -> W,
    ) -> Option<W> {
        let mut session = self;
        session.take_guarded(seal)
    }

    /// The NOTIFY and uncorrelated messages this connection has collected, in arrival order.
    ///
    /// This is the NOTIFY stream (e.g. `/ci/registeredDevices`, `/ro/values` changes); the
    /// post-CONNECT `/ro/*` responses are in [`Self::ro_values`]. The messages carry appliance body
    /// data and must never be logged.
    #[must_use]
    pub fn notifications(&self) -> &[IncomingMessage] {
        self.fsm.notifications()
    }

    /// The appliance's current values: the `/ro/*` RESPONSE messages this connection received.
    ///
    /// This is the post-CONNECT `/ro/allMandatoryValues` and `/ro/allDescriptionChanges` snapshot,
    /// the only source of state for an appliance that pushes no NOTIFY (a dishwasher in standby
    /// does not). See [`hc_proto::Handshake::ro_values`]. The messages carry appliance body data and
    /// must never be logged.
    #[must_use]
    pub fn ro_values(&self) -> &[IncomingMessage] {
        self.fsm.ro_values()
    }

    /// Milliseconds since the session started on tokio's (pausable) clock; the [`Correlator`]'s
    /// time base.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "elapsed ms since session start cannot approach u64::MAX in any real or test run; \
                  a fallible conversion here would have an unreachable error arm"
    )]
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// Drives the handshake to CONNECTED and consumes the post-CONNECT `/ro` payloads.
    ///
    /// # Errors
    ///
    /// [`TransportError`] on a crypto/parse/transport failure or a fatal `code`.
    pub async fn run_handshake(&mut self) -> Result<HandshakeOutcome, TransportError> {
        // Drive until CONNECTED *and* every registered GET is answered, so the post-CONNECT `/ro`
        // responses carrying the initial state are read too.
        while !self.handshake_settled() {
            match self.drive_one().await {
                Ok(()) => {}
                Err(error) => {
                    // Any failure is close-and-reconnect: close once and surface one error.
                    close_on(&self.tx).await;
                    return Err(error);
                }
            }
        }
        Ok(HandshakeOutcome {
            state: self.fsm.state(),
            saw_403: self.fsm.saw_403(),
            notifications: self.notifications,
        })
    }

    /// Whether the handshake phase is complete: CONNECTED with no request still outstanding.
    fn handshake_settled(&self) -> bool {
        self.fsm.state() == State::Connected && self.correlator.is_empty()
    }

    /// One receive → decode → correlate → feed-FSM → seal-and-send cycle.
    async fn drive_one(&mut self) -> Result<(), TransportError> {
        let message = self.recv_message().await?;
        // A RESPONSE is paired by `msgID`; an uncorrelated one still reaches the FSM's general
        // handler.
        if message.action == Action::Response
            && let Some(msg_id) = message.msg_id
        {
            self.correlator.resolve(msg_id);
        }
        let outputs = self.fsm.on_message(message, self.nonce.as_mut())?;
        self.notifications = self.fsm.notifications().len();
        for output in outputs {
            self.send_message(&output).await?;
        }
        Ok(())
    }

    /// Receives and decodes one data frame, enforcing the correlator's deadline.
    ///
    /// Control frames (ping/pong/close) are handled inline: a ping is ponged, a pong is ignored
    /// (no heartbeat is outstanding during the handshake), a close ends the connection.
    async fn recv_message(&mut self) -> Result<IncomingMessage, TransportError> {
        loop {
            let frame = self.recv_frame_before_deadline().await?;
            match frame {
                WsFrame::Ping(payload) => send_frame_on(&self.tx, WsFrame::Pong(payload)).await?,
                WsFrame::Pong(_) => {}
                WsFrame::Close => return Err(TransportError::Closed),
                data => return self.decoder.decode(data),
            }
        }
    }

    /// Awaits the next frame, returning [`TransportError::RequestTimeout`] if a registered request
    /// passes its deadline first.
    async fn recv_frame_before_deadline(&mut self) -> Result<WsFrame, TransportError> {
        if self.correlator.is_empty() {
            return self.stream.next_frame().await;
        }
        let budget = Duration::from_millis(hc_proto::RESPONSE_TIMEOUT_MS);
        match timeout(budget, self.stream.next_frame()).await {
            Ok(result) => result,
            Err(_elapsed) => {
                let now = self.now_ms();
                // Name the resource of the first request that timed out.
                let resource = self
                    .correlator
                    .timed_out(now)
                    .into_iter()
                    .next()
                    .and_then(|msg_id| self.correlator.resolve(msg_id))
                    .unwrap_or_else(|| "unknown".to_owned());
                Err(TransportError::RequestTimeout { resource })
            }
        }
    }

    /// Registers a `GET` for correlation, then seals and sends one outbound message under the TX
    /// lock.
    async fn send_message(&mut self, message: &OutgoingMessage) -> Result<(), TransportError> {
        if message.expects_response() {
            self.correlator
                .register(message.msg_id, &message.resource, self.now_ms());
        }
        send_message_on(&self.tx, message).await
    }

    /// Runs the steady-state loop with **no** notification sink: messages accumulate in the FSM and
    /// are read through [`Self::notifications`].
    ///
    /// # Errors
    ///
    /// [`TransportError::PongTimeout`] on a missed heartbeat pong; other [`TransportError`]s on a
    /// transport or decode failure.
    pub async fn run_steady_state(&mut self) -> Result<(), TransportError> {
        self.run_steady_state_with(None).await
    }

    /// Sends a WebSocket Close and flushes, ending the session deliberately.
    ///
    /// Errors are swallowed: the session is over either way.
    ///
    /// Takes `&mut self` so the returned future is `Send`; a captured `&Session` would not be,
    /// because neither `D` nor the boxed nonce source is `Sync`.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "an exclusive borrow is what keeps this future Send"
    )]
    pub async fn close(&mut self) {
        close_on(&self.tx).await;
    }

    /// Runs the steady-state loop, delivering each routed notification to `sink` as it arrives.
    ///
    /// Do not run this under a timeout and read [`Self::notifications`] between slices instead:
    /// the heartbeat state is local to this future, so cancelling it means no ping is ever sent and
    /// a wedged socket is never detected.
    ///
    /// With a sink, notifications are handed over and not retained, which bounds memory on a
    /// long-lived connection. Without one, they accumulate in the FSM.
    ///
    /// # Errors
    ///
    /// [`TransportError::PongTimeout`] on a missed heartbeat pong; other [`TransportError`]s on a
    /// transport or decode failure.
    pub async fn run_steady_state_with(
        &mut self,
        mut sink: Option<&mut dyn NotificationSink>,
    ) -> Result<(), TransportError> {
        let period = Duration::from_millis(HEARTBEAT_INTERVAL_MS);
        // First tick one interval out, so pings land "every 20s" rather than immediately.
        let mut interval = tokio::time::interval_at(Instant::now() + period, period);
        // If a whole interval elapses with a ping still unanswered, the peer is presumed dead.
        let mut awaiting_pong = false;
        // Send through a local clone of the TX `Arc` so the heartbeat branch does not borrow the
        // whole session alongside the recv branch's borrow of `self.stream`.
        let tx = Arc::clone(&self.tx);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if awaiting_pong {
                        close_on(&tx).await;
                        return Err(TransportError::PongTimeout);
                    }
                    send_frame_on(&tx, WsFrame::Ping(Vec::new())).await?;
                    awaiting_pong = true;
                }
                frame = self.stream.next_frame() => {
                    match frame? {
                        WsFrame::Pong(_) => awaiting_pong = false,
                        WsFrame::Ping(payload) => send_frame_on(&tx, WsFrame::Pong(payload)).await?,
                        WsFrame::Close => return Ok(()),
                        data => self.route_steady_notification(data, &mut sink)?,
                    }
                }
            }
        }
    }

    /// Decodes a steady-state data frame and routes it through the FSM's general handler.
    ///
    /// With a `sink`, the FSM's buffer is drained into it; without one, the buffer accumulates.
    fn route_steady_notification(
        &mut self,
        data: WsFrame,
        // By reference so the caller can reborrow it on every loop iteration.
        sink: &mut Option<&mut dyn NotificationSink>,
    ) -> Result<(), TransportError> {
        let message = self.decoder.decode(data)?;
        let _outputs = self.fsm.on_message(message, self.nonce.as_mut())?;
        match sink {
            Some(sink) => {
                // The first drain also delivers anything buffered during the handshake.
                let batch = self.fsm.take_notifications();
                self.notifications = self.notifications.saturating_add(batch.len());
                sink.accept(&batch);
            }
            None => self.notifications = self.fsm.notifications().len(),
        }
        Ok(())
    }
}

// Free functions over `&Arc<Mutex<Tx>>` rather than `&self` methods: `Session` is not `Sync`, so
// an `&self` future would not be `Send`.

/// Encodes and writes one message under the TX lock, without registering it.
async fn send_message_on<Si: FrameSink, E: FrameEncoder>(
    tx: &Arc<Mutex<Tx<Si, E>>>,
    message: &OutgoingMessage,
) -> Result<(), TransportError> {
    let mut guard = tx.lock().await;
    let frame = guard.encoder.encode(message)?;
    guard.sink.send_frame(frame).await
}

/// Sends one raw frame through a TX handle.
async fn send_frame_on<Si: FrameSink, E>(
    tx: &Arc<Mutex<Tx<Si, E>>>,
    frame: WsFrame,
) -> Result<(), TransportError> {
    let mut guard = tx.lock().await;
    guard.sink.send_frame(frame).await
}

/// Sends a close frame through a TX handle; errors are ignored.
async fn close_on<Si: FrameSink, E>(tx: &Arc<Mutex<Tx<Si, E>>>) {
    let mut guard = tx.lock().await;
    let _ignored = guard.sink.close().await;
    drop(guard);
}
