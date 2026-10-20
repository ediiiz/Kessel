//! A profile with unusable key material fails closed without contacting anything.
//!
//! The target's host is `192.0.2.1` (RFC 5737 TEST-NET-1), never a real appliance. The call is
//! wrapped in a short timeout: a dial would either block past it or return a connect-class reason,
//! and both fail the test. The key material is syntactic nonsense.
//!
//! The failure is a typed [`LiveTestError::KeyMaterial`], decided before any socket is opened, and
//! its reason string carries no dialled host: on the AES path the host is the `haId`, which must
//! not reach the SPA, the SSE inspector or a log line.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use std::time::Duration;

use hc_crypto::Secret;
use hc_profile::ConnectionType;
use server::live_transport::LiveConnectionTester;
use server::onboarding::{ConnectionTarget, ConnectionTester as _};

/// RFC 5737 TEST-NET-1. Documentation-only; never a real appliance.
const UNROUTABLE_HOST: &str = "192.0.2.1";

/// Long enough that a completed *local* decision is never mistaken for a timeout, far shorter than
/// any TCP connect budget.
const NO_DIAL_BUDGET: Duration = Duration::from_secs(2);

/// An AES-shaped target (`iv` present) whose `key`/`iv` are syntactic nonsense.
fn target_with_unusable_keys() -> ConnectionTarget {
    ConnectionTarget {
        host: UNROUTABLE_HOST.to_owned(),
        connection_type: ConnectionType::Aes,
        psk64: Secret::new("!!! not base64 !!!".to_owned()),
        iv64: Some(Secret::new("!!! not base64 either !!!".to_owned())),
        // `None` — the transport default. The key material is refused before any port is used, which
        // is the whole point of this test; an override here would be inert either way.
        port: None,
    }
}

#[tokio::test]
async fn connection_test_reports_failure_without_contacting_on_missing_keys() {
    let tester = LiveConnectionTester::new();
    let target = target_with_unusable_keys();

    let verdict = tokio::time::timeout(NO_DIAL_BUDGET, tester.test(&target)).await;

    // 1. It decided locally. A dial to TEST-NET-1 would not have come back inside this budget.
    let outcome = verdict.expect("the failure is decided locally, without dialling anything");

    // 2. It failed, rather than reporting a device it never reached as reachable.
    let reason = outcome.expect_err("unusable key material must not report success");

    // 3. With the typed key-material class — not a connect-class error, which would mean a socket.
    assert!(
        reason.contains("key material"),
        "the reason must name the key material as the cause, got: {reason}"
    );
    assert!(
        reason.contains("no connection was attempted"),
        "the reason must say that nothing was dialled, got: {reason}"
    );

    // 4. It carries no dialled host. On the AES path the host is the `haId`.
    assert!(
        !reason.contains(UNROUTABLE_HOST),
        "the operator-facing reason must not quote the dialled host, which is an appliance identifier, got: {reason}"
    );

    // 5. No handshake completed, so nothing was recorded.
    assert!(
        tester.last_outcome().is_none(),
        "no handshake ran, so there is no outcome to report"
    );
}

#[test]
fn a_transport_error_never_quotes_the_dialled_host() {
    // The redaction is case-insensitive because a URL parser may normalise the authority, and the
    // real AES host is a `BRAND-VIB-MAC` string whose case is not ours to assume.
    let redacted = server::live_transport::redact_host(
        "failed to lookup address information for ws://Fake-Host-0001:80/homeconnect (fake-host-0001)",
        "FAKE-HOST-0001",
    );
    assert!(
        !redacted.to_lowercase().contains("fake-host-0001"),
        "every occurrence of the host must be replaced, got: {redacted}"
    );
    assert_eq!(
        redacted.matches("<host>").count(),
        2,
        "both occurrences are replaced, got: {redacted}"
    );
    // An empty host must not match everywhere.
    assert_eq!(
        server::live_transport::redact_host("nothing to redact", ""),
        "nothing to redact",
        "an empty host redacts nothing"
    );
}

/// The number of `İ` characters that makes a naive offset drift exactly one needle long.
///
/// `İ` (U+0130) is two bytes and lowercases to three (`i` + U+0307), so each one shifts every
/// subsequent offset in the lowercased copy one byte ahead of the same text in the original. Fourteen
/// of them shift by 14 — the byte length of `fake-host-0001`.
const DESYNC_CHARS: usize = 14;

#[test]
fn redaction_holds_when_lowercasing_changes_the_byte_length() {
    // `redact_host` searches a lowercased copy and splices the original. That is only sound while
    // lowercasing preserves byte offsets, which holds only for ASCII. With the prefix below the
    // offsets run one needle ahead, so splicing `detail[..at]` would keep the whole string, host
    // included. Hosts are ASCII today; this is a robustness property for the function whose job is
    // keeping a dialled host out of an operator-facing string.
    let detail = "İ".repeat(DESYNC_CHARS) + " fake-host-0001";
    let redacted = server::live_transport::redact_host(&detail, "FAKE-HOST-0001");
    assert!(
        !redacted.to_lowercase().contains("fake-host-0001"),
        "the host must not survive a non-ASCII error string, got: {redacted}"
    );
    assert!(
        !redacted.is_empty(),
        "the operator must still be told something, got an empty reason"
    );
}
