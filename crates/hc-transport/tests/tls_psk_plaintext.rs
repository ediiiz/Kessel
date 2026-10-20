//! The TLS-PSK frame path carries plaintext UTF-8 JSON and does no app-layer crypto:
//!
//! - **compile-level** — the codec types are zero-sized (they carry no `hc_crypto::AesSession`/chain,
//!   unlike `aes_ws::AesEncoder`), asserted by the `const` blocks below;
//! - **behavioural** — a frame goes out over a real in-process TLS-PSK tunnel and comes back as a
//!   plaintext text frame whose bytes are the JSON verbatim.
//!
//! No appliance is contacted.

#![cfg(feature = "tls-psk")]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed asserts; library code may not"
    )
)]

mod tls_psk_support;

use std::time::Duration;

use hc_proto::{Action, OutgoingMessage, Version};
use hc_transport::codec::{FrameDecoder, FrameEncoder};
use hc_transport::tls_psk::{PlaintextDecoder, PlaintextEncoder};
use hc_transport::transport::{FrameSink, FrameStream, WsFrame};

use tls_psk_support::{client_psk, client_ws, echo_one, spawn_loopback};

// Compile-level proof: the plaintext codec carries no chain state — it is zero-sized, so it cannot
// hold an `AesSession`/`Sealer`/`Opener` (which are far larger). No `hc-crypto` on the frame path.
const _: () = assert!(
    std::mem::size_of::<PlaintextEncoder>() == 0,
    "the plaintext encoder must be zero-sized: no chain state, no AesSession"
);
const _: () = assert!(
    std::mem::size_of::<PlaintextDecoder>() == 0,
    "the plaintext decoder must be zero-sized: no chain state, no AesSession"
);

#[tokio::test]
async fn plaintext_utf8_over_tunnel_no_app_layer_crypto() {
    let (addr, server) = spawn_loopback(echo_one).await;
    let psk = client_psk();
    let (mut sink, mut stream) = client_ws(addr, &psk).await;

    let message = OutgoingMessage::bare(
        2_879_624_297,
        3_544_218_773,
        "/ci/services",
        Version::Int(1),
        Action::Get,
    );
    let expected_json = String::from_utf8(message.to_wire_bytes().unwrap()).unwrap();

    // Encode: the plaintext encoder emits a TEXT frame carrying the JSON verbatim — never a sealed
    // binary frame.
    let mut encoder = PlaintextEncoder::new();
    let frame = encoder.encode(&message).unwrap();
    match &frame {
        WsFrame::Text(text) => assert_eq!(
            *text, expected_json,
            "the text frame is the plaintext JSON, not ciphertext"
        ),
        other => panic!(
            "the plaintext transport must emit a TEXT frame, got a {} frame",
            other.kind()
        ),
    }

    // Round-trip over the real vendored-OpenSSL TLS-PSK tunnel: the server echoes it back.
    sink.send_frame(frame).await.unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(5), stream.next_frame())
        .await
        .expect("the echoed frame must arrive, not hang")
        .expect("a reply frame");

    // It came back as a plaintext UTF-8 text frame: the in-process WS server uses no hc-crypto, so it
    // could only read and echo a readable text frame — this is the "no app-layer crypto" proof.
    let WsFrame::Text(reply_text) = &reply else {
        panic!(
            "the reply must be a TEXT frame, got a {} frame",
            reply.kind()
        );
    };
    assert_eq!(
        *reply_text, expected_json,
        "the tunnel carried plaintext JSON in both directions"
    );

    // Decode: the plaintext decoder parses the text frame back to the message with no open step.
    let mut decoder = PlaintextDecoder::new();
    let back = decoder.decode(reply).unwrap();
    assert_eq!(
        back.resource.as_deref(),
        Some("/ci/services"),
        "the message round-trips through the no-crypto codec unchanged"
    );
    assert_eq!(back.action, Action::Get, "the action round-trips too");

    server.await.unwrap();
}
