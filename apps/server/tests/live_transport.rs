//! The real connection tester selects its transport by the presence of an `iv`.
//!
//! A profile carrying an `iv` selects the AES `ws://` transport; one without it selects TLS-PSK.
//! Asserted for all four appliance classes, plus a cross-check that each manifest's own
//! `connectionType` agrees with the `iv` rule.
//!
//! Every assertion runs against the scrubbed profile fixtures; no socket is opened.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

mod common;

use hc_profile::{ConnectionType, Profile};
use server::live_transport::{WireTransport, select_transport};
use server::onboarding::target_for;

#[test]
fn real_connection_tester_selects_transport_by_iv_presence() {
    // (fixture, the transport the `iv` rule must choose, the manifest's own declared word)
    let cases = [
        ("hob-aes.zip", WireTransport::Aes, ConnectionType::Aes),
        ("oven-tls.zip", WireTransport::TlsPsk, ConnectionType::Tls),
        (
            "dishwasher-tls.zip",
            WireTransport::TlsPsk,
            ConnectionType::Tls,
        ),
        ("fridge-tls.zip", WireTransport::TlsPsk, ConnectionType::Tls),
    ];

    for (fixture, expected, declared) in cases {
        let profile = Profile::from_zip_bytes(&common::fixture_zip_bytes(fixture))
            .unwrap_or_else(|error| panic!("{fixture} parses: {error}"));
        let target = target_for(&profile, None);

        // The rule's input, stated so a failure below is attributable: `iv` present iff AES.
        assert_eq!(
            target.iv64.is_some(),
            expected == WireTransport::Aes,
            "{fixture}: an `iv` must be present exactly on the AES path (tls-psk.md §1)"
        );

        assert_eq!(
            select_transport(&target),
            expected,
            "{fixture}: the daemon must select {expected:?} for this profile (tls-psk.md §1)"
        );

        // Cross-check, not the rule itself: the manifest's declared `connectionType` agrees. If
        // this fires, the two sources of truth have diverged.
        assert_eq!(
            target.connection_type, declared,
            "{fixture}: the manifest's connectionType must agree with the `iv` rule"
        );
    }
}
