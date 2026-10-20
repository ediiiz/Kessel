//! Message framing: [`hc_proto::OutgoingMessage`] to [`WsFrame`] and back.
//!
//! This is the only place the two protocols differ once bytes are moving: `aes_ws` seals/opens
//! through the AES chain over binary frames, `tls_psk` uses plain text frames. Encoder and decoder
//! are separate traits because the TX and RX chains sit behind separate locks.

use hc_proto::{IncomingMessage, OutgoingMessage};

use crate::error::TransportError;
use crate::transport::WsFrame;

/// Encodes an outbound message to a wire frame. Holds the TX chain state (for AES); the session
/// calls it under the TX lock so encode and write happen atomically.
pub trait FrameEncoder: Send {
    /// Serialises and (for AES) seals one message into the frame that goes on the wire.
    ///
    /// # Errors
    ///
    /// [`TransportError::Proto`] if the message will not serialise, or [`TransportError::Crypto`]
    /// if the seal fails.
    fn encode(&mut self, message: &OutgoingMessage) -> Result<WsFrame, TransportError>;
}

/// Decodes an inbound wire frame to a parsed message. Holds the RX-direction chain state (for AES).
///
/// The decoder owns the frame-kind policy: `aes_ws` rejects any non-binary data frame with
/// [`TransportError::UnexpectedFrame`]. Control frames never reach a decoder.
pub trait FrameDecoder: Send {
    /// Verifies/decrypts (for AES) and parses one data frame.
    ///
    /// # Errors
    ///
    /// [`TransportError::UnexpectedFrame`] on a wrong-kind data frame; [`TransportError::Crypto`]
    /// on a chain failure; [`TransportError::Proto`] on a parse failure.
    fn decode(&mut self, frame: WsFrame) -> Result<IncomingMessage, TransportError>;
}
