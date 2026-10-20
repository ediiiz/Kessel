//! Live TLS-PSK baseline tests.
//!
//! DEVICE SAFETY. These contact real appliances on the LAN. They are read-only: connect, complete the
//! handshake, read, never write. Nothing outside this file prevents a write, and `#[ignore]` is the
//! only thing keeping an ordinary `cargo test` from contacting them; `just live` passes `--ignored`
//! deliberately.
//!
//! Identity fallback order: try the empty identity first, then `"HCCOM_Local_App"` (the hint the
//! device advertises) on handshake failure.

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

use base64::Engine as _;
use hc_crypto::Secret;
use hc_proto::{DeviceIdentity, State};
use hc_transport::TransportError;
use hc_transport::tls_psk::{self, PskConfig, TlsPskSession};

/// Decodes a profile `psk64` (URL-safe base64, padding optional) to its raw bytes.
fn decode_psk(psk64: &str) -> Vec<u8> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(psk64.trim_end_matches('='))
        .expect("psk64 decodes as URL-safe base64")
}

/// Fixed client identity constants, never per-connection random.
fn app_identity() -> DeviceIdentity {
    DeviceIdentity {
        name: "hcpy".to_owned(),
        id: "0badcafe".to_owned(),
    }
}

/// Connects with the identity fallback: `""` first, then `"HCCOM_Local_App"`.
async fn connect_with_identity_fallback(
    host: &str,
    psk_bytes: &[u8],
) -> Result<TlsPskSession, TransportError> {
    let spec_aligned = PskConfig::new(b"".to_vec(), Secret::new(psk_bytes.to_vec()));
    match tls_psk::connect(
        host,
        tls_psk::DEFAULT_TLS_PORT,
        &spec_aligned,
        app_identity(),
    )
    .await
    {
        Ok(session) => Ok(session),
        // Retry with the identity hint the device advertises.
        Err(_first) => {
            let hint = PskConfig::new(b"HCCOM_Local_App".to_vec(), Secret::new(psk_bytes.to_vec()));
            tls_psk::connect(host, tls_psk::DEFAULT_TLS_PORT, &hint, app_identity()).await
        }
    }
}

/// Runs the read-only handshake and asserts CONNECTED. Never writes to the appliance.
async fn assert_connects(host: &str, psk64: &str) {
    let psk_bytes = decode_psk(psk64);
    let mut session = connect_with_identity_fallback(host, &psk_bytes)
        .await
        .expect("the TLS-PSK connection is established");
    let outcome = session
        .run_handshake()
        .await
        .expect("the handshake completes without error");
    assert_eq!(
        outcome.state,
        State::Connected,
        "the appliance TLS-PSK handshake reaches CONNECTED"
    );
}

#[tokio::test]
#[ignore = "live: contacts the real dishwasher; run deliberately via `just live`"]
async fn live_tls_dishwasher_connects() {
    // READ-ONLY.
    let host = std::env::var("KESSEL_DISHWASHER_HOST")
        .expect("KESSEL_DISHWASHER_HOST holds the dishwasher address");
    let psk64 = std::env::var("KESSEL_DISHWASHER_PSK64")
        .expect("KESSEL_DISHWASHER_PSK64 holds the dishwasher psk64");
    assert_connects(&host, &psk64).await;
}

#[tokio::test]
#[ignore = "live: contacts the real OVEN; run deliberately via `just live`"]
async fn live_tls_oven_connects() {
    // DEVICE SAFETY: this is an OVEN. READ-ONLY — connect + handshake + read, never a write.
    let host = std::env::var("KESSEL_OVEN_HOST").expect("KESSEL_OVEN_HOST holds the oven address");
    let psk64 = std::env::var("KESSEL_OVEN_PSK64").expect("KESSEL_OVEN_PSK64 holds the oven psk64");
    assert_connects(&host, &psk64).await;
}

#[tokio::test]
#[ignore = "live: contacts the real fridge; run deliberately via `just live`"]
async fn live_tls_fridge_connects() {
    // READ-ONLY.
    let host =
        std::env::var("KESSEL_FRIDGE_HOST").expect("KESSEL_FRIDGE_HOST holds the fridge address");
    let psk64 =
        std::env::var("KESSEL_FRIDGE_PSK64").expect("KESSEL_FRIDGE_PSK64 holds the fridge psk64");
    assert_connects(&host, &psk64).await;
}
