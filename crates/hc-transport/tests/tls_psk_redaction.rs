//! [`PskConfig`] carries the psk, which must never render through `Debug` (nor may the identity).
//! The test first shows an unredacted psk `Vec` would render its byte value, then that `PskConfig`'s
//! `Debug` withholds it.

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

use hc_crypto::Secret;
use hc_transport::tls_psk::PskConfig;

#[test]
fn psk_config_debug_never_renders_the_psk_or_identity() {
    // A distinctive psk byte (0xAB == 171) so a leak is unambiguous in the rendered output.
    let psk_bytes = vec![0xAB_u8; 32];
    let identity = b"HCCOM_Local_App".to_vec();

    // Non-vacuity: an unredacted Vec<u8> Debug *does* render the byte value — so absence below is
    // meaningful, not a rendering that never contained it.
    assert!(
        format!("{psk_bytes:?}").contains("171"),
        "sanity: an unredacted psk Vec renders its byte value"
    );

    let config = PskConfig::new(identity, Secret::new(psk_bytes));
    let debug = format!("{config:?}");
    let pretty = format!("{config:#?}");

    for rendered in [&debug, &pretty] {
        assert!(
            !rendered.contains("171"),
            "the psk bytes must never render through Debug: {rendered}"
        );
        assert!(
            !rendered.contains("HCCOM_Local_App"),
            "the identity string must not render verbatim through Debug: {rendered}"
        );
        assert!(
            rendered.contains("<redacted>"),
            "the psk field is present but redacted: {rendered}"
        );
    }
}
