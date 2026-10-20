//! The plaintext codec for the TLS-PSK transport.
//!
//! JSON goes over the wire as UTF-8 text frames with no app-layer crypto; TLS protects the tunnel.
//! There is no chain state, and this module does not depend on `hc-crypto`.

use hc_proto::{IncomingMessage, OutgoingMessage, ProtoError, parse};

use crate::codec::{FrameDecoder, FrameEncoder};
use crate::error::TransportError;
use crate::transport::WsFrame;

/// The TLS-PSK send-side codec: serialises an [`OutgoingMessage`] to a **text** WS frame.
///
/// Stateless: unlike [`crate::aes_ws::AesEncoder`] there is no chain to hold.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlaintextEncoder;

impl PlaintextEncoder {
    /// A fresh encoder. Stateless.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl FrameEncoder for PlaintextEncoder {
    fn encode(&mut self, message: &OutgoingMessage) -> Result<WsFrame, TransportError> {
        // `serde_json` output is always UTF-8; the error arm exists only so this cannot panic.
        let clear = message.to_wire_bytes()?;
        let text = String::from_utf8(clear)
            .map_err(|error| TransportError::Proto(ProtoError::Decode(error.to_string())))?;

        // No `wire_head_hex`: here the wire bytes are the plaintext body, which must never be
        // traced. No chain either, so `chain` is `none`.
        crate::trace::FrameTrace {
            direction: crate::trace::DIRECTION_TX,
            transport: crate::trace::TRANSPORT_TLS_PSK,
            frame_kind: "text",
            wire_bytes: text.len(),
            wire_head_hex: None,
            action: message.action.as_wire(),
            resource: Some(&message.resource),
            msg_id: Some(message.msg_id),
            code: message.code,
            chain: crate::trace::CHAIN_NONE,
        }
        .emit();

        Ok(WsFrame::Text(text))
    }
}

/// The TLS-PSK receive-side codec: parses a **text** WS frame into an [`IncomingMessage`].
///
/// The TLS-PSK transport is text-only: any non-text data frame is a hard error that drops the
/// socket.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlaintextDecoder;

impl PlaintextDecoder {
    /// A fresh decoder. Stateless.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl FrameDecoder for PlaintextDecoder {
    fn decode(&mut self, frame: WsFrame) -> Result<IncomingMessage, TransportError> {
        // Text-only; anything else drops the socket.
        let WsFrame::Text(text) = frame else {
            return Err(TransportError::UnexpectedFrame(frame.kind()));
        };
        let message = parse(text.as_bytes())?;

        // See the encoder for why there is no `wire_head_hex` or chain ordinal.
        crate::trace::FrameTrace {
            direction: crate::trace::DIRECTION_RX,
            transport: crate::trace::TRANSPORT_TLS_PSK,
            frame_kind: "text",
            wire_bytes: text.len(),
            wire_head_hex: None,
            action: message.action.as_wire(),
            resource: message.resource.as_deref(),
            msg_id: message.msg_id,
            code: message.code,
            chain: crate::trace::CHAIN_NONE,
        }
        .emit();

        Ok(message)
    }
}
