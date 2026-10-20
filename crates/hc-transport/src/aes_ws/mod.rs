//! The AES WebSocket transport: `ws://<host>:80/homeconnect` with the `hc-crypto` app-layer chain.
//!
//! Outbound frames are `ct ‖ mac` in a binary WS frame. Inbound frames must be binary, at least
//! 32 bytes and block-aligned; any non-binary data frame is a hard error that drops the socket.
//! `tokio-tungstenite` runs without its TLS features, so this path links no TLS stack.

use hc_crypto::{Opener, Role, SealedFrame, Sealer, SessionKeys};
use hc_proto::{
    DeviceIdentity, Handshake, HandshakeConfig, IncomingMessage, OutgoingMessage, parse,
};
use hc_proto::{RandomNonce, Transport, appliance_url_at};

use crate::codec::{FrameDecoder, FrameEncoder};
use crate::error::TransportError;
use crate::session::{Endpoints, Session};
use crate::transport::{WsFrame, WsSink, WsStream, split};

/// The AES send-side codec: seals an [`OutgoingMessage`] into a binary WS frame.
///
/// Holds the TX [`Sealer`] (CBC + rolling-HMAC chain) and the pad source, separately from the RX
/// [`AesDecoder`] so each can sit behind its own lock.
pub struct AesEncoder<P> {
    sealer: Sealer,
    pad: P,
    /// Frames sealed so far on this connection; the value of [`crate::trace::KEY_CHAIN`]. A count,
    /// never chain material: the rolling HMAC and CBC IV are key-derived and must not be traced.
    chain_seq: u64,
}

impl<P> core::fmt::Debug for AesEncoder<P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // `sealer`'s `Debug` redacts the chain; the pad source is not rendered.
        f.debug_struct("AesEncoder")
            .field("sealer", &self.sealer)
            .finish_non_exhaustive()
    }
}

impl<P: hc_crypto::PadSource + Send> AesEncoder<P> {
    /// A fresh TX codec with the chain at its start-of-connection state.
    #[must_use]
    pub fn new(keys: SessionKeys, pad: P) -> Self {
        Self {
            sealer: Sealer::new(keys, Role::Client.seal_direction()),
            pad,
            chain_seq: 0,
        }
    }

    /// This direction's rolling HMAC; 16 zero bytes on a fresh codec.
    #[must_use]
    pub const fn last_hmac(&self) -> [u8; hc_crypto::MAC_LEN] {
        self.sealer.last_hmac()
    }
}

impl<P: hc_crypto::PadSource + Send> FrameEncoder for AesEncoder<P> {
    fn encode(&mut self, message: &OutgoingMessage) -> Result<WsFrame, TransportError> {
        let clear = message.to_wire_bytes()?;
        let sealed: SealedFrame = self.sealer.seal(&clear, &mut self.pad)?;
        let wire = sealed.into_wire();

        // Only the message header is traced, never `clear`, which carries device identifiers.
        let head = crate::trace::head_hex(&wire);
        crate::trace::FrameTrace {
            direction: crate::trace::DIRECTION_TX,
            transport: crate::trace::TRANSPORT_AES,
            frame_kind: "binary",
            wire_bytes: wire.len(),
            wire_head_hex: Some(&head),
            action: message.action.as_wire(),
            resource: Some(&message.resource),
            msg_id: Some(message.msg_id),
            code: message.code,
            chain: &self.chain_seq.to_string(),
        }
        .emit();
        self.chain_seq = self.chain_seq.saturating_add(1);

        Ok(WsFrame::Binary(wire))
    }
}

/// The AES receive-side codec: opens a binary WS frame into a parsed [`IncomingMessage`].
///
/// Holds the RX [`Opener`]. A non-binary data frame is a hard error.
pub struct AesDecoder {
    opener: Opener,
    /// Frames opened so far on this connection. A count, never chain material.
    chain_seq: u64,
}

impl core::fmt::Debug for AesDecoder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // `opener`'s `Debug` redacts the chain.
        f.debug_struct("AesDecoder")
            .field("opener", &self.opener)
            .field("chain_seq", &self.chain_seq)
            .finish()
    }
}

impl AesDecoder {
    /// A fresh RX codec with the chain at its start-of-connection state.
    #[must_use]
    pub fn new(keys: SessionKeys) -> Self {
        Self {
            opener: Opener::new(keys, Role::Client.open_direction()),
            chain_seq: 0,
        }
    }

    /// This direction's rolling HMAC.
    #[must_use]
    pub const fn last_hmac(&self) -> [u8; hc_crypto::MAC_LEN] {
        self.opener.last_hmac()
    }
}

impl FrameDecoder for AesDecoder {
    fn decode(&mut self, frame: WsFrame) -> Result<IncomingMessage, TransportError> {
        // AES is binary-only; anything else drops the socket.
        let WsFrame::Binary(bytes) = frame else {
            return Err(TransportError::UnexpectedFrame(frame.kind()));
        };
        let opened = self.opener.open(&bytes)?;
        let message = parse(opened.plaintext.as_bytes())?;

        // Only the decoded header is traced, never the plaintext, which carries device identifiers.
        let head = crate::trace::head_hex(&bytes);
        crate::trace::FrameTrace {
            direction: crate::trace::DIRECTION_RX,
            transport: crate::trace::TRANSPORT_AES,
            frame_kind: "binary",
            wire_bytes: bytes.len(),
            wire_head_hex: Some(&head),
            action: message.action.as_wire(),
            resource: message.resource.as_deref(),
            msg_id: message.msg_id,
            code: message.code,
            chain: &self.chain_seq.to_string(),
        }
        .emit();
        self.chain_seq = self.chain_seq.saturating_add(1);

        Ok(message)
    }
}

/// The byte stream under a real `ws://` socket.
type WsTcp = tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>;

/// The concrete AES session over the real `tokio-tungstenite` transport.
pub type AesSession<P> = Session<WsSink<WsTcp>, WsStream<WsTcp>, AesEncoder<P>, AesDecoder>;

/// Dials `ws://<host>:<port>/homeconnect` and returns a ready-to-drive [`Session`].
///
/// `port` overrides the default of 80. The URL is what tungstenite dials, so this is the only place
/// an AES port comes from.
///
/// # Errors
///
/// [`TransportError`] if the URL is malformed or the WebSocket connection fails.
pub async fn connect<P: hc_crypto::PadSource + Send + 'static>(
    host: &str,
    port: Option<u16>,
    keys: SessionKeys,
    identity: DeviceIdentity,
    pad: P,
) -> Result<AesSession<P>, TransportError> {
    let url = appliance_url_at(host, Transport::Aes, port)?;
    let (stream, _response) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|error| TransportError::WebSocket(error.to_string()))?;
    let (sink, source) = split(stream);
    let endpoints = Endpoints::new(
        sink,
        source,
        AesEncoder::new(keys.clone(), pad),
        AesDecoder::new(keys),
    );
    let fsm = Handshake::new(HandshakeConfig { identity });
    Ok(Session::new(endpoints, fsm, Box::new(RandomNonce)))
}

/// The production nonce source, re-exported for callers.
pub use hc_proto::RandomNonce as ProductionNonce;
