//! The transport failure taxonomy, shared by the AES and TLS-PSK transports.
//!
//! Every variant is fatal to the current connection: once a frame cannot be decoded the chain
//! position is uncertain, so the caller must reconnect (with a fresh [`hc_crypto::AesSession`])
//! rather than continue.

use thiserror::Error;

/// Everything the transport can refuse to do.
///
/// Variants carry lengths, resources and frame-kind names but never key material, plaintext or
/// ciphertext. The wrapped [`hc_crypto::CryptoError`] and [`hc_proto::ProtoError`] also redact.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TransportError {
    /// The peer closed, or the injected transport ran dry, cleanly or not.
    #[error("transport closed / no more frames")]
    Closed,

    /// A data frame of the wrong kind for this transport arrived (AES is binary-only). Carries the
    /// kind seen, never its payload.
    #[error("unexpected {0} frame; this transport drops the socket on it")]
    UnexpectedFrame(&'static str),

    /// A crypto failure from the `hc-crypto` chain: bad MAC, bad padding, non-UTF-8, misaligned
    /// frame.
    #[error(transparent)]
    Crypto(#[from] hc_crypto::CryptoError),

    /// A protocol, parse or FSM failure from `hc-proto`.
    #[error(transparent)]
    Proto(#[from] hc_proto::ProtoError),

    /// A registered request went unanswered past [`hc_proto::RESPONSE_TIMEOUT_MS`].
    #[error(
        "request for {resource} timed out after {}ms",
        hc_proto::RESPONSE_TIMEOUT_MS
    )]
    RequestTimeout {
        /// The resource of the request that was not answered.
        resource: String,
    },

    /// A heartbeat ping was not answered by a pong before the next interval; the connection is
    /// presumed dead.
    #[error("heartbeat pong not received before the deadline")]
    PongTimeout,

    /// An I/O or WebSocket-protocol error from `tokio-tungstenite`, stringified so no buffer
    /// contents survive.
    #[error("websocket transport error: {0}")]
    WebSocket(String),
}
