//! Binding and serving the local API.
//!
//! The listener binds all interfaces ([`api_address`]) because Home Assistant's ingress proxy
//! connects to the container's own IP on the internal network, so a loopback listener is
//! unreachable from it. The API stays off the LAN because `kessel/config.yaml` declares no `ports:`
//! key; `tests/loopback_bind.rs` asserts that. Adding a `ports:` key would publish a write-capable
//! API to the LAN.
//!
//! There is no host setting: [`bind_api_listener`] takes a port only.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use tokio::net::TcpListener;

/// The port the daemon listens on unless told otherwise.
///
/// 8099 is the port Home Assistant add-ons conventionally use for an ingress web UI.
pub const DEFAULT_PORT: u16 = 8099;

/// The environment variable that overrides [`DEFAULT_PORT`], following the `KESSEL_*` prefix.
pub const PORT_ENV: &str = "KESSEL_PORT";

/// The address the API is served on: all IPv4 interfaces (`0.0.0.0`), at `port`.
///
/// See the module docs for why it is not loopback.
#[must_use]
pub const fn api_address(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)
}

/// The configured port: `KESSEL_PORT` when it parses, [`DEFAULT_PORT`] otherwise.
///
/// A malformed value logs a warning and falls back rather than aborting, so a typo in an add-on
/// option does not leave the user with no UI.
#[must_use]
pub fn configured_port() -> u16 {
    std::env::var(PORT_ENV).map_or(DEFAULT_PORT, |raw| {
        raw.parse::<u16>().unwrap_or_else(|_| {
            tracing::warn!(
                env = PORT_ENV,
                default = DEFAULT_PORT,
                "port is not a number; falling back to the default"
            );
            DEFAULT_PORT
        })
    })
}

/// Binds the API listener on all interfaces at `port` (`0` lets the OS choose).
///
/// # Errors
///
/// The underlying `bind` error, such as `AddrInUse` for a taken port.
pub async fn bind_api_listener(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(api_address(port)).await
}
