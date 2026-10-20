//! Live baseline: the real hob answers the AES handshake and does not return `/ci/services`
//! code 403.
//!
//! DEVICE SAFETY. The hob is a cooktop. This test is read-only: it completes the handshake and never
//! writes. Nothing outside this file prevents a write, and `#[ignore]` is the only thing keeping an
//! ordinary `cargo test` from contacting it; `just live` passes `--ignored` deliberately.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed asserts; library code may not"
    )
)]

use rand::SeedableRng as _;

use hc_crypto::SessionKeys;
use hc_proto::{DeviceIdentity, State};
use hc_transport::{TransportError, aes_ws};

#[tokio::test]
#[ignore = "live: contacts the real hob, a cooktop; run deliberately via `just live`"]
async fn live_aes_hob_handshake_no_403() {
    // Real key material and host come from the appliance profile, supplied through the environment.
    let host = std::env::var("KESSEL_HOB_HOST").expect("KESSEL_HOB_HOST holds the hob address");
    let psk64 = std::env::var("KESSEL_HOB_PSK64").expect("KESSEL_HOB_PSK64 holds the hob psk64");
    let iv64 = std::env::var("KESSEL_HOB_IV64").expect("KESSEL_HOB_IV64 holds the hob iv64");
    let keys = SessionKeys::from_base64(&psk64, &iv64).expect("hob key material decodes");

    // Fixed client identity constants, never per-connection random.
    let identity = DeviceIdentity {
        name: "hcpy".to_owned(),
        id: "0badcafe".to_owned(),
    };

    // Production pad randomness: an OS-seeded CSPRNG.
    let pad = rand::rngs::StdRng::from_os_rng();

    let mut session = aes_ws::connect(&host, None, keys, identity, pad)
        .await
        .expect("the ws:// connection to the hob is established");

    // READ-ONLY: run_handshake reads and completes; it issues no write to the appliance.
    match session.run_handshake().await {
        Ok(outcome) => {
            assert_eq!(
                outcome.state,
                State::Connected,
                "the hob handshake reaches CONNECTED"
            );
            // The regression asserted directly: no 403 on /ci/services.
            assert!(
                !outcome.saw_403,
                "REGRESSION: the hob answered /ci/services with code 403"
            );
        }
        // A fatal `code` on /ci/services surfaces as a Proto error carrying the code; a 403 there is
        // the regression. Fail loudly and print the observed error either way.
        Err(TransportError::Proto(error)) => {
            panic!("hob handshake failed (this is the live check for the 403 regression): {error}");
        }
        Err(other) => panic!("hob handshake transport failure: {other}"),
    }
}
