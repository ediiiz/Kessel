//! The handshake driver — ties a [`Socket`], the `hc-crypto` AES chain, a [`Clock`], a pad source
//! and a [`NonceSource`] to the pure [`Handshake`] FSM.
//!
//! The driver is synchronous and blocking, for replay tests. The async transport runs its own
//! receive loop over the same [`Handshake`], [`Correlator`] and message types.
//!
//! Each step receives a frame, decrypts, parses and feeds it to the FSM, then seals and sends
//! whatever the FSM emits, registering each `GET` for correlation. It stops at
//! [`State::Connected`] or [`State::Failed`].

use core::fmt;

use hc_crypto::{AesSession, PadSource};

use crate::correlation::{Clock, Correlator};
use crate::error::ProtoError;
use crate::fsm::{Handshake, NonceSource, State};
use crate::message::{Action, OutgoingMessage, parse};
use crate::socket::{Socket, SocketError};

/// The result of driving a handshake to a terminal state.
#[derive(Debug, Clone, Copy)]
pub struct HandshakeOutcome {
    /// The FSM's terminal state ([`State::Connected`] on success).
    pub state: State,
    /// Whether any handshake response carried `code == 403`.
    pub saw_403: bool,
}

/// Anything that can go wrong driving the handshake.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DriverError {
    /// A protocol/parse/FSM error.
    #[error(transparent)]
    Proto(#[from] ProtoError),
    /// A transport error.
    #[error(transparent)]
    Socket(#[from] SocketError),
    /// A crypto error from the `hc-crypto` chain.
    #[error(transparent)]
    Crypto(#[from] hc_crypto::CryptoError),
}

/// The injected runtime sources for a [`Driver`]: the clock, the nonce source and the pad source.
#[derive(Debug)]
pub struct DriverSources<C, N, P> {
    /// The monotonic clock (correlation deadlines).
    pub clock: C,
    /// The `/ci/authentication` nonce source.
    pub nonce: N,
    /// The seal pad source.
    pub pad: P,
}

/// Drives the handshake over an injected socket, crypto chain, clock, pad source and nonce source.
///
/// `Debug` is hand-written because the embedded FSM's notifications can carry device identifiers.
pub struct Driver<S, C, N, P> {
    socket: S,
    session: AesSession,
    clock: C,
    nonce: N,
    pad: P,
    fsm: Handshake,
    correlator: Correlator,
}

impl<S, C, N, P> fmt::Debug for Driver<S, C, N, P> {
    /// `fsm` and `session` render through their own redacting `Debug`s. The socket and sources are
    /// omitted, which also drops the `Debug` bounds a derive would impose.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Driver")
            .field("session", &self.session)
            .field("fsm", &self.fsm)
            .field("correlator", &self.correlator)
            .finish_non_exhaustive()
    }
}

impl<S: Socket, C: Clock, N: NonceSource, P: PadSource> Driver<S, C, N, P> {
    /// Assembles a driver. `session` must already hold the connection's [`hc_crypto::SessionKeys`]
    /// in the client role.
    pub fn new(
        socket: S,
        session: AesSession,
        sources: DriverSources<C, N, P>,
        fsm: Handshake,
    ) -> Self {
        let DriverSources { clock, nonce, pad } = sources;
        Self {
            socket,
            session,
            clock,
            nonce,
            pad,
            fsm,
            correlator: Correlator::new(),
        }
    }

    /// The socket, for inspecting what was sent.
    #[must_use]
    pub const fn socket(&self) -> &S {
        &self.socket
    }

    /// The FSM, for inspecting notifications and state.
    #[must_use]
    pub const fn handshake(&self) -> &Handshake {
        &self.fsm
    }

    /// Drives the handshake to a terminal state.
    ///
    /// # Errors
    ///
    /// [`DriverError`] on a crypto, parse, transport or fatal-`code` failure.
    pub fn run_handshake(&mut self) -> Result<HandshakeOutcome, DriverError> {
        while !matches!(self.fsm.state(), State::Connected | State::Failed) {
            self.step()?;
        }
        Ok(HandshakeOutcome {
            state: self.fsm.state(),
            saw_403: self.fsm.saw_403(),
        })
    }

    /// One receive → decrypt → parse → feed → send cycle.
    fn step(&mut self) -> Result<(), DriverError> {
        let wire = self.socket.recv()?;
        let opened = self.session.open(&wire)?;
        let message = parse(opened.plaintext.as_bytes())?;
        // A RESPONSE is paired by msgID; an uncorrelated one still goes to the FSM.
        if message.action == Action::Response
            && let Some(msg_id) = message.msg_id
        {
            self.correlator.resolve(msg_id);
        }
        let outputs = self.fsm.on_message(message, &mut self.nonce)?;
        for output in outputs {
            self.send_output(&output)?;
        }
        Ok(())
    }

    /// Registers a `GET` for correlation, then seals and sends one outbound message.
    fn send_output(&mut self, output: &OutgoingMessage) -> Result<(), DriverError> {
        if output.expects_response() {
            self.correlator
                .register(output.msg_id, &output.resource, self.clock.now_ms());
        }
        let clear = output.to_wire_bytes()?;
        let sealed = self.session.seal(&clear, &mut self.pad)?;
        self.socket.send(&sealed.into_wire())?;
        Ok(())
    }
}
