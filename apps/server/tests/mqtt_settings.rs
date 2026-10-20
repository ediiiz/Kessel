//! The broker settings: optional, and credential-safe.
//!
//! A daemon with no broker configured runs no MQTT task, opens no connection and publishes
//! nothing; `None` is the state that delivers that.
//!
//! Edition 2024 makes `std::env::set_var` `unsafe` and these crates forbid unsafe code, so the
//! parser is a pure function of a lookup and `from_env` is the one-line wrapper that supplies
//! `std::env::var`. Nothing here opens a socket.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use std::collections::BTreeMap;

use server::mqtt::MqttSettings;

/// A lookup over a fixed map — the stand-in for the process environment.
fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let map: BTreeMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    move |key: &str| map.get(key).cloned()
}

#[test]
fn mqtt_settings_are_absent_until_a_broker_host_is_set() {
    assert!(
        MqttSettings::from_lookup(lookup(&[])).is_none(),
        "with nothing set there is no broker, and the daemon must run no MQTT task \
         at all — no connection, no publish"
    );

    // Every other variable set, but no host: still absent. The host is the switch, so a
    // half-configured broker cannot look configured.
    assert!(
        MqttSettings::from_lookup(lookup(&[
            ("KESSEL_MQTT_PORT", "1884"),
            ("KESSEL_MQTT_USERNAME", "kessel"),
            ("KESSEL_MQTT_BASE", "somewhere"),
        ]))
        .is_none(),
        "a port and a username without a host is not a broker"
    );

    let settings = MqttSettings::from_lookup(lookup(&[("KESSEL_MQTT_HOST", "192.0.2.2")]))
        .expect("a host alone is enough to configure a broker");
    assert_eq!(settings.host, "192.0.2.2");
}

#[test]
fn the_password_never_appears_in_a_debug_rendering() {
    // A credential, and the daemon renders its configuration into the log at startup, so it is
    // wrapped in `hc_crypto::Secret<T>`, whose `Debug` redacts.
    let settings = MqttSettings::from_lookup(lookup(&[
        ("KESSEL_MQTT_HOST", "192.0.2.2"),
        ("KESSEL_MQTT_USERNAME", "kessel"),
        ("KESSEL_MQTT_PASSWORD", "s3cret-do-not-print"),
    ]))
    .expect("configured");

    let rendered = format!("{settings:?}");
    assert!(
        !rendered.contains("s3cret-do-not-print"),
        "the password must not reach a Debug rendering: {rendered}"
    );
    // The alternate form, which a `{:#?}` in a log line or a panic message produces.
    let alternate = format!("{settings:#?}");
    assert!(
        !alternate.contains("s3cret-do-not-print"),
        "nor the alternate Debug form: {alternate}"
    );
    // The username is not a secret and stays visible: an operator debugging an auth failure needs to
    // see which user was used.
    assert!(
        rendered.contains("kessel"),
        "the username is not a credential and remains renderable: {rendered}"
    );
}

#[test]
fn defaults_apply_when_only_a_host_is_given() {
    let settings =
        MqttSettings::from_lookup(lookup(&[("KESSEL_MQTT_HOST", "broker")])).expect("configured");
    assert_eq!(
        settings.port,
        MqttSettings::DEFAULT_PORT,
        "the default port is the named constant, not a literal repeated at the call site"
    );
    assert_eq!(
        settings.base,
        MqttSettings::DEFAULT_BASE,
        "and the topic root likewise"
    );
    assert!(
        settings.username.is_none() && settings.password.is_none(),
        "an anonymous broker is the default; credentials are opt-in"
    );
}

#[test]
fn an_unparseable_port_is_refused_rather_than_defaulted() {
    // Silently falling back to 1883 would leave a working-looking daemon talking to the wrong port.
    // `serve.rs`'s `configured_port` falls back for KESSEL_PORT, where a bad bind fails visibly;
    // here the failure would be a silent non-connection.
    assert!(
        MqttSettings::from_lookup(lookup(&[
            ("KESSEL_MQTT_HOST", "broker"),
            ("KESSEL_MQTT_PORT", "not-a-port"),
        ]))
        .is_none(),
        "a broker host with an unparseable port must not silently become the default port"
    );
}
