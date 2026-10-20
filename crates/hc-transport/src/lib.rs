//! `hc-transport`: async WebSocket transports and the transport-agnostic session loop they drive.
//!
//! `aes_ws` dials `ws://host:80` and runs the `hc-crypto` AES app-layer chain; `tls_psk`
//! (`feature = "tls-psk"`) dials `wss://host:443` over OpenSSL TLS-PSK with no app-layer crypto.
//! They differ only in their byte transport ([`transport`]) and message framing ([`codec`]); the
//! FSM drive, correlation, timeouts, heartbeat and serialised sends live in [`session`].
//!
//! The session drives [`hc_proto::Handshake`] and [`hc_proto::Correlator`] directly rather than
//! the synchronous `hc_proto::Driver`. All I/O is injectable, so tests need no network.

#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "test asserts may panic; library code may not"
    )
)]

pub mod aes_ws;
pub mod codec;
pub mod error;
pub mod session;
pub mod trace;
pub mod transport;

// Feature-gated so the default build links no OpenSSL.
#[cfg(feature = "tls-psk")]
pub mod tls_psk;

pub use codec::{FrameDecoder, FrameEncoder};
pub use error::TransportError;
pub use session::{Endpoints, HandshakeOutcome, NotificationSink, Sender, Session, WRITE_RESOURCE};
pub use trace::FrameTrace;
pub use transport::{FrameSink, FrameStream, WsFrame};

/// The crate's own name, printed in the server's startup banner.
pub const CRATE_NAME: &str = "hc-transport";

#[cfg(test)]
mod tests {
    use super::CRATE_NAME;

    #[test]
    fn crate_name_is_stable() {
        assert_eq!(
            CRATE_NAME, "hc-transport",
            "crate name is part of the workspace layout"
        );
    }
}
