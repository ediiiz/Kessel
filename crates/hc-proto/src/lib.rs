//! `hc-proto` — the protocol layer: the `{sID, msgID, resource, version, action, data, code}`
//! message model, defensive parsing, the handshake FSM and request/response correlation.
//!
//! The crate is I/O-free: no sockets, no HTTP/WebSocket dependency, no async runtime. The FSM is
//! driven through the [`Socket`] and [`Clock`] traits; `hc-transport` supplies the real ones.
//!
//! Outgoing messages are serialised with a fixed field order, never through `serde_json::Value`,
//! because the firmware parses JSON objects positionally.

#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic; library code may not"
    )
)]

mod codes;
mod correlation;
mod driver;
mod error;
mod fsm;
mod message;
mod socket;
mod updates;
mod url;

pub use codes::ResponseCode;
pub use correlation::{Clock, Correlator, RESPONSE_TIMEOUT_MS};
pub use driver::{Driver, DriverError, DriverSources, HandshakeOutcome};
pub use error::ProtoError;
pub use fsm::{Handshake, HandshakeConfig, MsgIdAllocator, NonceSource, RandomNonce, State};
pub use message::{
    Action, Body, DeviceIdentity, DeviceType, IncomingMessage, OutgoingMessage, Version, WriteItem,
    WriteValue, parse, service_key,
};
pub use socket::{FakeSocket, Socket, SocketError};
pub use updates::{DescriptionUpdate, ValueUpdate, apply_updates};
pub use url::{Transport, appliance_url, appliance_url_at};

/// The crate's own name, printed in the server's startup banner.
pub const CRATE_NAME: &str = "hc-proto";

#[cfg(test)]
mod tests {
    use super::CRATE_NAME;

    #[test]
    fn crate_name_is_stable() {
        assert_eq!(
            CRATE_NAME, "hc-proto",
            "crate name is part of the workspace layout"
        );
    }
}
