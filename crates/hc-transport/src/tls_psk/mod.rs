//! The TLS-PSK WebSocket transport: `wss://<host>:443/homeconnect` over vendored OpenSSL, with no
//! app-layer crypto (`feature = "tls-psk"`).
//!
//! Provides the plaintext codec, the OpenSSL TLS-PSK tunnel ([`tls`]), and [`connect`], which dials
//! the tunnel and upgrades to WebSocket over it. The session loop is the shared
//! [`crate::session`]. `set_psk_client_callback` is a safe API, so no `unsafe` is needed.

pub mod codec;
pub mod tls;

pub use codec::{PlaintextDecoder, PlaintextEncoder};
pub use tls::{PskConfig, TlsPskError, psk_connector, tls_connect_stream};

use tokio::net::TcpStream;

use crate::error::TransportError;
use crate::session::{Endpoints, Session};
use crate::transport::{WsSink, WsStream, split};

use hc_proto::{
    DeviceIdentity, Handshake, HandshakeConfig, RandomNonce, Transport, appliance_url_at,
};

/// The TLS-PSK default port.
pub const DEFAULT_TLS_PORT: u16 = 443;

/// The concrete session type [`connect`] produces.
///
/// The shared [`Session`] over an async OpenSSL `SslStream`, with the plaintext codec pair.
pub type TlsPskSession = Session<
    WsSink<tokio_openssl::SslStream<TcpStream>>,
    WsStream<tokio_openssl::SslStream<TcpStream>>,
    PlaintextEncoder,
    PlaintextDecoder,
>;

/// The linked OpenSSL version string, e.g. `"OpenSSL 3.6.3 9 Jun 2026"`.
#[must_use]
pub fn openssl_version() -> &'static str {
    openssl::version::version()
}

/// The linked OpenSSL `OPENSSLDIR` string.
///
/// A vendored build reports the upstream compile-time default (`/usr/local/ssl`), not a path under
/// `target/`, so this does not tell a vendored build apart; use [`links_vendored_openssl`].
#[must_use]
pub fn openssl_dir() -> &'static str {
    openssl::version::dir()
}

/// Whether this binary **statically** links the vendored OpenSSL — the host-independent proof that
/// `features = ["vendored"]` took effect.
///
/// A vendored build links `libssl.a`/`libcrypto.a` statically, so no `libssl.so`/`libcrypto.so`
/// is mapped into the process.
#[must_use]
pub fn links_vendored_openssl() -> bool {
    // Without /proc static linking cannot be proven, so report false.
    std::fs::read_to_string("/proc/self/maps")
        .is_ok_and(|maps| !maps.contains("libssl.so") && !maps.contains("libcrypto.so"))
}

/// Dials a TLS-PSK tunnel to `host:port`, WS-upgrades over it, and returns a ready-to-drive
/// [`TlsPskSession`].
///
/// `psk` carries the TLS pre-shared key and identity; `identity` is the app-layer
/// [`DeviceIdentity`] the handshake answers `/ei/initialValues` with.
///
/// # Errors
///
/// [`TransportError`] if the TLS handshake, the TCP dial, or the WebSocket upgrade fails.
pub async fn connect(
    host: &str,
    port: u16,
    psk: &PskConfig,
    identity: DeviceIdentity,
) -> Result<TlsPskSession, TransportError> {
    // TLS first, then the WebSocket upgrade inside it.
    let stream = tls_connect_stream(host, port, psk)
        .await
        .map_err(|error| TransportError::WebSocket(error.to_string()))?;

    // Scheme `ws`, not `wss`: the stream is already TLS and tungstenite must not add a second
    // layer. The port in the Host header is cosmetic (the real dial used `port` above) but is kept
    // consistent with the socket.
    let url = appliance_url_at(host, Transport::TlsPsk, Some(port))?.replacen("wss://", "ws://", 1);
    let (ws, _response) = tokio_tungstenite::client_async(url, stream)
        .await
        .map_err(|error| TransportError::WebSocket(error.to_string()))?;

    let (sink, source) = split(ws);
    let endpoints = Endpoints::new(
        sink,
        source,
        PlaintextEncoder::new(),
        PlaintextDecoder::new(),
    );
    let fsm = Handshake::new(HandshakeConfig { identity });
    Ok(Session::new(endpoints, fsm, Box::new(RandomNonce)))
}
