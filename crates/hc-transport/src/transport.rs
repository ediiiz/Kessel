//! The byte transport: WebSocket frames and the send/receive halves a session drives.
//!
//! [`crate::session`] is generic over a [`FrameSink`] and a [`FrameStream`]. Both transports use
//! the same WebSocket framing; only the data frame kind differs, which is a [`crate::codec`]
//! concern. Send and receive are split so the TX and RX chains sit behind separate locks and the
//! read loop is never serialised against sends.

use async_trait::async_trait;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

use crate::error::TransportError;

/// A WebSocket frame, reduced to the kinds this transport distinguishes.
///
/// The [`crate::codec`] decides which data kind is legal; the control kinds carry the session's
/// heartbeat and shutdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsFrame {
    /// A binary data frame, the AES transport's only legal data kind.
    Binary(Vec<u8>),
    /// A text data frame, the TLS-PSK transport's data kind.
    Text(String),
    /// A control ping.
    Ping(Vec<u8>),
    /// A control pong.
    Pong(Vec<u8>),
    /// A close frame.
    Close,
}

impl WsFrame {
    /// A short, payload-free name for this frame's kind, for [`TransportError::UnexpectedFrame`].
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Binary(_) => "binary",
            Self::Text(_) => "text",
            Self::Ping(_) => "ping",
            Self::Pong(_) => "pong",
            Self::Close => "close",
        }
    }
}

/// The send half of a transport. The session holds this behind the TX lock.
#[async_trait]
pub trait FrameSink: Send {
    /// Writes one frame to the wire.
    ///
    /// # Errors
    ///
    /// [`TransportError::Closed`] or [`TransportError::WebSocket`] if the frame cannot be written.
    async fn send_frame(&mut self, frame: WsFrame) -> Result<(), TransportError>;

    /// Sends a close frame and flushes. A second call may error; callers ignore that.
    ///
    /// # Errors
    ///
    /// [`TransportError::Closed`] or [`TransportError::WebSocket`].
    async fn close(&mut self) -> Result<(), TransportError>;
}

/// The receive half of a transport.
#[async_trait]
pub trait FrameStream: Send {
    /// The next inbound frame.
    ///
    /// # Errors
    ///
    /// [`TransportError::Closed`] when the peer is gone; [`TransportError::WebSocket`] on a
    /// protocol error.
    async fn next_frame(&mut self) -> Result<WsFrame, TransportError>;
}

/// The real send half: a split `tokio-tungstenite` sink.
///
/// Generic over the byte stream `S`: plain TCP for `aes_ws`, an OpenSSL `SslStream` for `tls_psk`.
pub struct WsSink<S> {
    inner: SplitSink<WebSocketStream<S>, Message>,
}

/// The real receive half: a split `tokio-tungstenite` stream.
pub struct WsStream<S> {
    inner: SplitStream<WebSocketStream<S>>,
}

impl<S> core::fmt::Debug for WsSink<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The split sink holds a send buffer; wire bytes never go through `Debug`.
        f.debug_struct("WsSink").finish_non_exhaustive()
    }
}

impl<S> core::fmt::Debug for WsStream<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WsStream").finish_non_exhaustive()
    }
}

/// Splits a connected `tokio-tungstenite` stream into the [`WsSink`]/[`WsStream`] pair the session
/// drives.
pub fn split<S>(stream: WebSocketStream<S>) -> (WsSink<S>, WsStream<S>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (sink, source) = stream.split();
    (WsSink { inner: sink }, WsStream { inner: source })
}

#[async_trait]
impl<S> FrameSink for WsSink<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    async fn send_frame(&mut self, frame: WsFrame) -> Result<(), TransportError> {
        let message = match frame {
            WsFrame::Binary(bytes) => Message::Binary(bytes.into()),
            WsFrame::Text(text) => Message::Text(text.into()),
            WsFrame::Ping(payload) => Message::Ping(payload.into()),
            WsFrame::Pong(payload) => Message::Pong(payload.into()),
            WsFrame::Close => Message::Close(None),
        };
        self.inner
            .send(message)
            .await
            .map_err(|error| TransportError::WebSocket(error.to_string()))
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        self.inner
            .close()
            .await
            .map_err(|error| TransportError::WebSocket(error.to_string()))
    }
}

#[async_trait]
impl<S> FrameStream for WsStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    async fn next_frame(&mut self) -> Result<WsFrame, TransportError> {
        loop {
            let message = self
                .inner
                .next()
                .await
                .ok_or(TransportError::Closed)?
                .map_err(|error| TransportError::WebSocket(error.to_string()))?;
            // The codec, not the transport, decides which data kind is legal.
            return Ok(match message {
                Message::Binary(bytes) => WsFrame::Binary(bytes.to_vec()),
                Message::Text(text) => WsFrame::Text(text.to_string()),
                Message::Ping(payload) => WsFrame::Ping(payload.to_vec()),
                Message::Pong(payload) => WsFrame::Pong(payload.to_vec()),
                Message::Close(_) => WsFrame::Close,
                // A raw `Frame` never surfaces from a read; skip anything else and read again.
                Message::Frame(_) => continue,
            });
        }
    }
}
