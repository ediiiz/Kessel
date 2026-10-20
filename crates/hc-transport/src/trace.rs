//! Per-frame trace events for the frame inspector.
//!
//! Each codec emits exactly one event per data frame, on the success path, because the codec is
//! the one place that holds the wire bytes and the decoded message at once. The server treats an
//! event as a frame when its fields contain [`KEY_DIRECTION`].
//!
//! # Fields
//!
//! The keys are fixed as `KEY_*` constants because consumers read them as untyped strings.
//!
//! | key | values |
//! |---|---|
//! | [`KEY_DIRECTION`] | [`DIRECTION_RX`] \| [`DIRECTION_TX`] |
//! | [`KEY_TRANSPORT`] | [`TRANSPORT_AES`] \| [`TRANSPORT_TLS_PSK`] |
//! | [`KEY_FRAME_KIND`] | `binary` \| `text` |
//! | [`KEY_WIRE_BYTES`] | decimal |
//! | [`KEY_WIRE_HEAD_HEX`] | lowercase hex of the first ≤ [`WIRE_HEX_PREVIEW_BYTES`] wire bytes; AES only |
//! | [`KEY_ACTION`] | `GET` \| `POST` \| `RESPONSE` \| `NOTIFY` \| `DELETE` |
//! | [`KEY_RESOURCE`] | e.g. `/ci/services` |
//! | [`KEY_MSG_ID`] | decimal; a `RESPONSE` reuses its request's `msgID`, so this pairs them |
//! | [`KEY_CODE`] | decimal, present only when the frame carries one |
//! | [`KEY_CHAIN`] | decimal chain ordinal on AES, [`CHAIN_NONE`] on TLS-PSK |
//!
//! An absent value is an absent key, never an empty string, so "no `code`" is distinguishable
//! from `code: 0`.
//!
//! # What is never emitted
//!
//! - The decoded `data` body: it carries device identifiers (`deviceID`, `haId`, `mac`,
//!   `ipAddress`). Only the header (action, resource, msgID, code) is traced.
//! - Wire hex on TLS-PSK, where the wire bytes are the plaintext body. On AES they are ciphertext,
//!   and the prefix stops before the trailing HMAC tag.
//! - Chain material. [`KEY_CHAIN`] is a count of frames the chain has advanced (`0` on a fresh
//!   connection); the rolling HMAC and CBC IV are key-derived and must never be logged.
//!
//! The handshake step is not available at the codec boundary, and failed frames emit nothing.
//!
//! Events are emitted at `TRACE` level on target [`FRAME_TARGET`].

use crate::transport::WsFrame;

/// The `tracing` target every frame event carries.
pub const FRAME_TARGET: &str = "hc_transport::frame";

/// The `message` every frame event carries; the per-frame detail is in the fields.
pub const FRAME_MESSAGE: &str = "frame";

/// The field whose presence makes an event a frame for the server's classifier.
pub const KEY_DIRECTION: &str = "direction";
/// Which transport the frame crossed.
pub const KEY_TRANSPORT: &str = "transport";
/// The WebSocket data-frame kind ([`WsFrame::kind`]).
pub const KEY_FRAME_KIND: &str = "frame_kind";
/// The on-wire payload length in bytes, decimal.
pub const KEY_WIRE_BYTES: &str = "wire_bytes";
/// A bounded lowercase-hex prefix of the on-wire payload. AES only — see the module docs.
pub const KEY_WIRE_HEAD_HEX: &str = "wire_head_hex";
/// The wire verb (`hc_proto::Action::as_wire`).
pub const KEY_ACTION: &str = "action";
/// The wire `resource` path.
pub const KEY_RESOURCE: &str = "resource";
/// The wire `msgID`, decimal; the correlation id.
pub const KEY_MSG_ID: &str = "msg_id";
/// The wire `code`, decimal. Absent unless the frame carried one.
pub const KEY_CODE: &str = "code";
/// The app-layer chain ordinal, or [`CHAIN_NONE`].
pub const KEY_CHAIN: &str = "chain";

/// A frame **received from** the appliance.
pub const DIRECTION_RX: &str = "rx";
/// A frame **sent to** the appliance.
pub const DIRECTION_TX: &str = "tx";

/// The AES `ws://` transport.
pub const TRANSPORT_AES: &str = "aes";
/// The TLS-PSK `wss://` transport.
pub const TRANSPORT_TLS_PSK: &str = "tls-psk";

/// [`KEY_CHAIN`]'s value on a transport with no app-layer chain (TLS-PSK).
pub const CHAIN_NONE: &str = "none";

/// How many leading wire bytes [`KEY_WIRE_HEAD_HEX`] renders: one AES block.
///
/// Bounded so per-frame dumps do not push other events off the bounded trace bus.
pub const WIRE_HEX_PREVIEW_BYTES: usize = 16;

/// One frame's worth of inspector payload, ready to [`emit`](FrameTrace::emit).
///
/// Every field is a fixed vocabulary token or a value that cannot hold a device identifier.
#[derive(Debug, Clone, Copy)]
pub struct FrameTrace<'a> {
    /// [`DIRECTION_RX`] or [`DIRECTION_TX`].
    pub direction: &'static str,
    /// [`TRANSPORT_AES`] or [`TRANSPORT_TLS_PSK`].
    pub transport: &'static str,
    /// [`WsFrame::kind`] of the data frame.
    pub frame_kind: &'static str,
    /// The on-wire payload length in bytes.
    pub wire_bytes: usize,
    /// A bounded hex prefix of the on-wire payload, or `None` where the wire bytes are the body.
    pub wire_head_hex: Option<&'a str>,
    /// The wire verb.
    pub action: &'static str,
    /// The wire `resource`, when the frame carried one.
    pub resource: Option<&'a str>,
    /// The wire `msgID`, when the frame carried one.
    pub msg_id: Option<u64>,
    /// The wire `code`, when the frame carried one.
    pub code: Option<i64>,
    /// The app-layer chain ordinal, or [`CHAIN_NONE`].
    pub chain: &'a str,
}

impl FrameTrace<'_> {
    /// Emits this frame to `tracing`.
    ///
    /// `tracing` needs literal field names, so the names below must match the `KEY_*` constants by
    /// hand; a unit test pins the constants and `tests/frame_events.rs` pins the emitted keys.
    pub fn emit(&self) {
        tracing::trace!(
            target: FRAME_TARGET,
            direction = self.direction,
            transport = self.transport,
            frame_kind = self.frame_kind,
            wire_bytes = self.wire_bytes,
            wire_head_hex = self.wire_head_hex,
            action = self.action,
            resource = self.resource,
            msg_id = self.msg_id,
            code = self.code,
            chain = self.chain,
            "{FRAME_MESSAGE}"
        );
    }
}

/// Renders the first [`WIRE_HEX_PREVIEW_BYTES`] of `bytes` as lowercase hex.
#[must_use]
pub fn head_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(WIRE_HEX_PREVIEW_BYTES * 2);
    for byte in bytes.iter().take(WIRE_HEX_PREVIEW_BYTES) {
        out.push(nibble(byte >> 4));
        out.push(nibble(byte & 0x0F));
    }
    out
}

/// One lowercase hex digit, computed without table indexing.
fn nibble(value: u8) -> char {
    let digit = if value < 10 {
        b'0'.saturating_add(value)
    } else {
        b'a'.saturating_add(value).saturating_sub(10)
    };
    char::from(digit)
}

/// The on-wire payload length of a data frame, in bytes.
///
/// A close frame answers `0`.
#[must_use]
pub const fn wire_bytes(frame: &WsFrame) -> usize {
    match frame {
        WsFrame::Binary(bytes) | WsFrame::Ping(bytes) | WsFrame::Pong(bytes) => bytes.len(),
        WsFrame::Text(text) => text.len(),
        WsFrame::Close => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CHAIN_NONE, DIRECTION_RX, DIRECTION_TX, FrameTrace, KEY_ACTION, KEY_CHAIN, KEY_CODE,
        KEY_DIRECTION, KEY_FRAME_KIND, KEY_MSG_ID, KEY_RESOURCE, KEY_TRANSPORT, KEY_WIRE_BYTES,
        KEY_WIRE_HEAD_HEX, WIRE_HEX_PREVIEW_BYTES, head_hex, wire_bytes,
    };
    use crate::transport::WsFrame;

    /// Pins the `KEY_*` constants to their spellings. The emitted keys are pinned separately in
    /// `tests/frame_events.rs`.
    #[test]
    fn the_key_constants_match_the_emitted_field_names() {
        // Kept in the same order as `FrameTrace::emit`'s macro body, so a reader can diff them.
        assert_eq!(
            [
                KEY_DIRECTION,
                KEY_TRANSPORT,
                KEY_FRAME_KIND,
                KEY_WIRE_BYTES,
                KEY_WIRE_HEAD_HEX,
                KEY_ACTION,
                KEY_RESOURCE,
                KEY_MSG_ID,
                KEY_CODE,
                KEY_CHAIN,
            ],
            [
                "direction",
                "transport",
                "frame_kind",
                "wire_bytes",
                "wire_head_hex",
                "action",
                "resource",
                "msg_id",
                "code",
                "chain",
            ],
            "the KEY_* constants and `FrameTrace::emit`'s field names have drifted — the server's \
             frame classifier depends on these exact values"
        );
    }

    #[test]
    fn the_direction_vocabulary_is_the_merged_one() {
        // The server and web inspector use the same "rx"/"tx" vocabulary.
        assert_eq!((DIRECTION_RX, DIRECTION_TX), ("rx", "tx"));
    }

    #[test]
    fn head_hex_is_lowercase_and_bounded() {
        assert_eq!(head_hex(&[0x0F, 0xA0, 0xFF]), "0fa0ff");
        let long = vec![0xAB_u8; WIRE_HEX_PREVIEW_BYTES * 4];
        assert_eq!(
            head_hex(&long).len(),
            WIRE_HEX_PREVIEW_BYTES * 2,
            "the preview must stay bounded: an unbounded per-frame dump would lap hc-core's \
             bounded trace bus and cost other events"
        );
    }

    #[test]
    fn wire_bytes_counts_the_payload() {
        assert_eq!(wire_bytes(&WsFrame::Binary(vec![0; 48])), 48);
        assert_eq!(wire_bytes(&WsFrame::Text("{}".to_owned())), 2);
        assert_eq!(wire_bytes(&WsFrame::Close), 0);
    }

    /// A frame with no `msgID`, `resource` or `code` is expressible.
    #[test]
    fn a_chainless_frame_is_expressible() {
        let frame = FrameTrace {
            direction: DIRECTION_RX,
            transport: super::TRANSPORT_TLS_PSK,
            frame_kind: "text",
            wire_bytes: 2,
            wire_head_hex: None,
            action: "NOTIFY",
            resource: None,
            msg_id: None,
            code: None,
            chain: CHAIN_NONE,
        };
        frame.emit();
        assert_eq!(frame.chain, "none");
    }
}
