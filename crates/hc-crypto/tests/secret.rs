//! `Secret<T>` redaction.
//!
//! Secrets must not escape through `Debug` (logs, panics), `Display` (formatted strings, MQTT
//! payloads) or `serde` (SSE frames).

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic; library code may not"
    )
)]

use hc_crypto::Secret;

/// A payload that must never appear in any rendering. Not a real device value.
const PAYLOAD: &str = "kessel-test-psk-do-not-log";

/// A struct that contains a secret: the realistic leak path, since a derived `Debug` renders
/// every field.
#[derive(Debug)]
#[allow(
    dead_code,
    reason = "the fields exist only to be rendered by the derived Debug impl under test"
)]
struct Profile {
    host: &'static str,
    psk: Secret<String>,
}

/// The serde equivalent: what an SSE frame or a config dump looks like.
#[derive(serde::Serialize)]
struct Frame {
    resource: &'static str,
    psk: Secret<String>,
    iv: Secret<Vec<u8>>,
}

#[test]
fn secret_debug_redacts() {
    let secret = Secret::new(PAYLOAD.to_owned());

    let rendered = format!("{secret:?}");

    assert!(
        !rendered.contains(PAYLOAD),
        "Debug leaked the payload: {rendered}"
    );
    assert!(
        rendered.contains("redacted"),
        "Debug must say it redacted something, got: {rendered}"
    );

    // The alternate (`{:#?}`) form is what `tracing`'s field capture and most derived `Debug`
    // impls reach for, so it has to redact too.
    let alternate = format!("{secret:#?}");
    assert!(
        !alternate.contains(PAYLOAD),
        "alternate Debug leaked the payload: {alternate}"
    );

    // Nesting inside a derived `Debug` is the realistic leak path.
    let nested = format!(
        "{:?}",
        Profile {
            host: "example.invalid",
            psk: Secret::new(PAYLOAD.to_owned()),
        }
    );
    assert!(
        !nested.contains(PAYLOAD),
        "derived Debug on a containing struct leaked the payload: {nested}"
    );
}

#[test]
fn secret_display_redacts() {
    let secret = Secret::new(PAYLOAD.to_owned());

    let rendered = secret.to_string();

    assert!(
        !rendered.contains(PAYLOAD),
        "Display leaked the payload: {rendered}"
    );
    assert!(
        rendered.contains("redacted"),
        "Display must say it redacted something, got: {rendered}"
    );

    // Binary payloads take the same route (psk and iv are bytes, not strings).
    let bytes = Secret::new(vec![0xDE_u8, 0xAD, 0xBE, 0xEF]);
    let rendered_bytes = bytes.to_string();
    assert!(
        !rendered_bytes.contains("222"),
        "Display leaked a byte of the payload: {rendered_bytes}"
    );

    // The escape hatch has to exist and has to be explicit at the call site.
    assert_eq!(
        secret.expose_secret(),
        PAYLOAD,
        "expose_secret is the single, deliberate way out"
    );
}

#[test]
fn secret_serialize_redacts() {
    let secret = Secret::new(PAYLOAD.to_owned());

    let json = serde_json::to_string(&secret).expect("Secret<String> serialises");
    assert!(
        !json.contains(PAYLOAD),
        "serde leaked the payload: {json} — SSE frames are serialised"
    );
    assert!(
        json.contains("redacted"),
        "serialised form must be the redaction marker, got: {json}"
    );

    // The realistic case: a secret as a field of a serialised frame/config struct.
    let frame = serde_json::to_string(&Frame {
        resource: "/ci/services",
        psk: Secret::new(PAYLOAD.to_owned()),
        iv: Secret::new(vec![1_u8, 2, 3, 4]),
    })
    .expect("frame serialises");
    assert!(
        !frame.contains(PAYLOAD),
        "serde leaked the payload from a struct field: {frame}"
    );
    assert!(
        !frame.contains("[1,2,3,4]"),
        "serde leaked the raw iv bytes: {frame}"
    );
    assert!(
        frame.contains("/ci/services"),
        "non-secret fields must still serialise normally: {frame}"
    );

    // Deserialisation must still work — config files carry real secrets inbound.
    let parsed: Secret<String> =
        serde_json::from_str("\"inbound-value\"").expect("Secret<String> deserialises");
    assert_eq!(
        parsed.expose_secret(),
        "inbound-value",
        "redaction is outbound-only; reading a secret in must round-trip"
    );
}
