//! Shared in-process TLS-PSK loopback harness for the `tls_psk` integration tests.
//!
//! The server is an in-process OpenSSL TLS-PSK endpoint on an ephemeral `127.0.0.1` port; the client
//! connects through the real vendored OpenSSL. No appliance is contacted.
//!
//! The handshake replay feeds the `plaintext_utf8` fields of `fixtures/aes/hob-session.json` as text
//! frames. The JSON/FSM sequence is transport-independent, so the AES corpus stands in for a TLS
//! capture.

#![allow(
    dead_code,
    unreachable_pub,
    reason = "each integration test binary compiles this module independently and uses a subset"
)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "the test harness may panic on setup failure; library code may not"
)]

use std::net::SocketAddr;
use std::pin::Pin;

use futures_util::{SinkExt as _, StreamExt as _};
use hc_crypto::Secret;
use hc_proto::DeviceIdentity;
use openssl::error::ErrorStack;
use openssl::ssl::{Ssl, SslContext, SslMethod, SslVersion};
use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

use hc_transport::tls_psk::{PskConfig, tls_connect_stream};
use hc_transport::transport::{WsSink, WsStream, split};

/// The shared loopback pre-shared key: arbitrary 32 bytes, not a real appliance psk.
pub const TEST_PSK: [u8; 32] = [0x5A; 32];

const CORPUS_JSON: &str = include_str!("../../../../fixtures/aes/hob-session.json");

/// The concrete server-side WebSocket stream type.
pub type WsServer = WebSocketStream<tokio_openssl::SslStream<TcpStream>>;

/// The client's [`PskConfig`] for the loopback: empty identity plus the shared [`TEST_PSK`], wrapped
/// in [`Secret`] so it can only exit via `expose_secret`.
#[must_use]
pub fn client_psk() -> PskConfig {
    PskConfig::new(b"".to_vec(), Secret::new(TEST_PSK.to_vec()))
}

/// The app-layer identity the handshake FSM answers `/ei/initialValues` with: the corpus client, so
/// the replay is realistic (the server ignores it).
#[must_use]
pub fn corpus_identity() -> DeviceIdentity {
    let corpus: Value = serde_json::from_str(CORPUS_JSON).expect("corpus parses");
    let client = corpus.get("client").expect("corpus has a client");
    DeviceIdentity {
        name: client
            .get("device_name")
            .and_then(Value::as_str)
            .expect("device_name")
            .to_owned(),
        id: client
            .get("device_id")
            .and_then(Value::as_str)
            .expect("device_id")
            .to_owned(),
    }
}

/// The `plaintext_utf8` of every `rx` frame, in receive order — the JSON the appliance sent, replayed
/// as text frames over the tunnel.
#[must_use]
pub fn rx_plaintext_frames() -> Vec<String> {
    let corpus: Value = serde_json::from_str(CORPUS_JSON).expect("corpus parses");
    corpus
        .get("frames")
        .and_then(Value::as_array)
        .expect("frames array")
        .iter()
        .filter(|frame| frame.get("direction").and_then(Value::as_str) == Some("rx"))
        .map(|frame| {
            frame
                .get("plaintext_utf8")
                .and_then(Value::as_str)
                .expect("rx frame has plaintext_utf8")
                .to_owned()
        })
        .collect()
}

/// Builds the server-side TLS-PSK [`Ssl`]: PSK **server** callback, cipher list `"PSK"`, seclevel 0,
/// TLS 1.2 — the mirror of the client connector, and no certificate (pure PSK).
fn server_ssl() -> Ssl {
    let mut ctx = SslContext::builder(SslMethod::tls_server()).expect("server ctx");
    ctx.set_min_proto_version(Some(SslVersion::TLS1_2))
        .expect("min version");
    ctx.set_max_proto_version(Some(SslVersion::TLS1_2))
        .expect("max version");
    ctx.set_security_level(0);
    ctx.set_cipher_list("PSK").expect("cipher list");
    ctx.set_psk_server_callback(|_ssl, _identity, psk_out| {
        let dst = psk_out
            .get_mut(..TEST_PSK.len())
            .ok_or_else(ErrorStack::get)?;
        dst.copy_from_slice(&TEST_PSK);
        Ok(TEST_PSK.len())
    });
    Ssl::new(&ctx.build()).expect("server Ssl")
}

/// Binds an ephemeral `127.0.0.1` port, accepts **one** TLS-PSK + WebSocket connection, and runs
/// `handler` on the upgraded stream. Returns the address to dial and the server task handle.
pub async fn spawn_loopback<F, Fut>(handler: F) -> (SocketAddr, JoinHandle<()>)
where
    F: FnOnce(WsServer) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let handle = tokio::spawn(async move {
        let (tcp, _peer) = listener.accept().await.expect("accept tcp");
        let mut tls = tokio_openssl::SslStream::new(server_ssl(), tcp).expect("server SslStream");
        Pin::new(&mut tls)
            .accept()
            .await
            .expect("server TLS accept");
        let ws = tokio_tungstenite::accept_async(tls)
            .await
            .expect("server WS accept");
        handler(ws).await;
    });
    (addr, handle)
}

/// Dials the loopback through the **real vendored OpenSSL**, WS-upgrades over the tunnel, and returns
/// the split [`WsSink`]/[`WsStream`] halves, below the codec. Lets the plaintext test drive the
/// codec directly; the full [`hc_transport::tls_psk::connect`] path is the loopback test's job.
pub async fn client_ws(
    addr: SocketAddr,
    psk: &PskConfig,
) -> (
    WsSink<tokio_openssl::SslStream<TcpStream>>,
    WsStream<tokio_openssl::SslStream<TcpStream>>,
) {
    let host = addr.ip().to_string();
    let stream = tls_connect_stream(&host, addr.port(), psk)
        .await
        .expect("client TLS-PSK handshake");
    // Scheme `ws` (not `wss`): the WS upgrade runs over the already-TLS stream.
    let url = format!("ws://{addr}/homeconnect");
    let (ws, _response) = tokio_tungstenite::client_async(url, stream)
        .await
        .expect("client WS upgrade");
    split(ws)
}

/// Server behavior: echo exactly one frame back to the client (same kind), then close. Used to prove
/// the frame crosses the tunnel as-is.
pub async fn echo_one(mut ws: WsServer) {
    if let Some(Ok(message)) = ws.next().await {
        ws.send(message).await.expect("echo send");
    }
    let _ = ws.close(None).await;
}

/// Server behavior: replay every `rx` `plaintext_utf8` as a TLS **text** frame, then drain the
/// client's frames until it closes (so its sends never backpressure).
pub async fn replay_handshake(mut ws: WsServer) {
    for text in rx_plaintext_frames() {
        ws.send(Message::Text(text.into()))
            .await
            .expect("replay send");
    }
    while let Some(Ok(message)) = ws.next().await {
        if message.is_close() {
            break;
        }
    }
}
