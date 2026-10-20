//! The Home Assistant Supervisor as a source of MQTT configuration.
//!
//! Under the add-on, Home Assistant already runs a broker and hands its credentials to any add-on
//! that asks. These tests cover the payload parser, the refusals, and the precedence rule that
//! decides when autodiscovery is consulted. There is no Supervisor in CI, so the HTTP call itself
//! (`hassio::fetch_mqtt_broker`) is not exercised. Nothing here opens a socket.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use server::config::{StoredConfig, StoredMqtt};
use server::hassio::{self, HassioError};

/// The shape the Supervisor answers `GET /services/mqtt` with.
///
/// The envelope (`result` / `data`) and field names follow the Supervisor's published API; the
/// parser is strict about the envelope and treats an unrecognised answer as an error.
const SERVICE_OK: &str = r#"{
  "result": "ok",
  "data": {
    "addon": "core_mosquitto",
    "host": "core-mosquitto",
    "port": 1883,
    "ssl": false,
    "username": "addons",
    "password": "br0ker-p4ss",
    "protocol": "3.1.1"
  }
}"#;

#[test]
fn the_supervisor_mqtt_service_becomes_a_broker() {
    let broker = hassio::parse_mqtt_service(SERVICE_OK.as_bytes()).expect("a well-formed service");

    assert_eq!(broker.host, "core-mosquitto");
    assert_eq!(broker.port, 1883);
    assert_eq!(broker.username.as_deref(), Some("addons"));
    assert!(
        broker.password.is_some(),
        "the credential the Supervisor handed over must be carried, or the connection is refused \
         by the broker"
    );
}

#[test]
fn the_broker_credential_is_not_rendered_by_debug() {
    let broker = hassio::parse_mqtt_service(SERVICE_OK.as_bytes()).expect("a well-formed service");

    let rendered = format!("{broker:?}");
    assert!(
        !rendered.contains("br0ker-p4ss"),
        "the broker password must be `Secret`-wrapped: this struct is logged at startup"
    );
}

#[test]
fn a_tls_broker_is_refused_rather_than_dialled_in_the_clear() {
    let ssl = SERVICE_OK.replace("\"ssl\": false", "\"ssl\": true");

    let error = hassio::parse_mqtt_service(ssl.as_bytes()).expect_err("TLS must be refused");
    assert!(
        matches!(error, HassioError::BrokerWantsTls),
        "a broker that wants TLS must be refused by name, not connected to in plaintext: the root \
         manifest drops rumqttc's `use-rustls` deliberately, so this build cannot speak it. Got \
         {error:?}"
    );
}

#[test]
fn an_error_envelope_is_not_a_broker() {
    let refused = r#"{"result":"error","message":"No MQTT service available"}"#;

    let error = hassio::parse_mqtt_service(refused.as_bytes()).expect_err("not a service");
    assert!(
        matches!(error, HassioError::Refused { .. }),
        "the Supervisor's own refusal must survive as a refusal — an operator with no broker \
         installed needs to be told that, not told the answer was malformed. Got {error:?}"
    );
}

#[test]
fn a_truncated_answer_is_malformed_rather_than_a_guess() {
    let error = hassio::parse_mqtt_service(b"{\"result\":\"ok\"").expect_err("not JSON");
    assert!(matches!(error, HassioError::Malformed(_)), "got {error:?}");
}

// =================================================================================================
// Precedence: autodiscovery fills a gap, it never overrules the operator.
// =================================================================================================

/// A stored config with a broker the operator typed themselves.
fn stored_with_broker() -> StoredConfig {
    StoredConfig {
        mqtt: Some(StoredMqtt {
            host: "192.0.2.2".to_owned(),
            port: Some(1884),
            username: None,
            password: None,
            base: None,
            discovery: None,
        }),
        ..StoredConfig::default()
    }
}

#[test]
fn a_typed_broker_is_never_replaced_by_the_supervisor() {
    assert!(
        !hassio::should_autodiscover(&stored_with_broker(), |_| None),
        "an operator who typed a broker must get that broker. Autodiscovery is a default, not an \
         override — the opposite rule would make a wrong stored value unfixable from the UI."
    );
}

#[test]
fn an_environment_broker_is_never_replaced_by_the_supervisor() {
    assert!(
        !hassio::should_autodiscover(&StoredConfig::default(), |key| {
            (key == server::mqtt::MqttSettings::HOST_ENV).then(|| "192.0.2.7".to_owned())
        }),
        "`KESSEL_MQTT_HOST` is the escape hatch from a bad stored value (`server::config`'s module \
         docs); autodiscovery must not close it"
    );
}

#[test]
fn nothing_configured_is_what_autodiscovery_is_for() {
    assert!(
        hassio::should_autodiscover(&StoredConfig::default(), |_| None),
        "a fresh add-on install has no stored broker and no environment: this is the case the \
         operator should not have to type anything for"
    );
}

#[test]
fn the_operator_can_switch_autodiscovery_off() {
    assert!(
        !hassio::should_autodiscover(&StoredConfig::default(), |key| {
            (key == hassio::AUTODISCOVER_ENV).then(|| "false".to_owned())
        }),
        "the add-on option must be able to turn this off — a second broker on the LAN is a real \
         deployment, and it is reached by leaving the Supervisor's out of it"
    );
}

#[test]
fn autodiscovery_is_on_unless_the_word_says_otherwise() {
    for word in ["true", "1", "yes", "on", "", "nonsense"] {
        assert!(
            hassio::autodiscovery_enabled(|key| {
                (key == hassio::AUTODISCOVER_ENV).then(|| word.to_owned())
            }),
            "{word:?} must not silently disable autodiscovery: the cost of a typo here is a broker \
             that is never found, which looks exactly like a broken add-on"
        );
    }
    for word in ["false", "0", "no", "off", "FALSE"] {
        assert!(
            !hassio::autodiscovery_enabled(|key| {
                (key == hassio::AUTODISCOVER_ENV).then(|| word.to_owned())
            }),
            "{word:?} must disable autodiscovery"
        );
    }
}

#[test]
fn a_discovered_broker_keeps_the_operators_topic_root_and_discovery_mode() {
    let stored = StoredConfig {
        mqtt: Some(StoredMqtt {
            // No host: what the settings page saves when only the topic root was touched.
            // `StoredMqtt` cannot exist without one, so the base and the mode arrive separately, as
            // `settings_from` takes them.
            host: String::new(),
            port: None,
            username: None,
            password: None,
            base: Some("haus".to_owned()),
            discovery: Some("curated".to_owned()),
        }),
        ..StoredConfig::default()
    };
    let broker = hassio::parse_mqtt_service(SERVICE_OK.as_bytes()).expect("a well-formed service");

    let settings = broker.into_settings(&stored);

    assert_eq!(settings.host, "core-mosquitto");
    assert_eq!(settings.port, 1883);
    assert_eq!(
        settings.base, "haus",
        "the Supervisor supplies the broker, never the topic layout: a discovered broker must not \
         quietly move every topic back to the default root"
    );
    assert_eq!(
        format!("{:?}", settings.discovery),
        "Curated",
        "the discovery mode is the operator's choice and is orthogonal to where the broker came from"
    );
}

/// The add-on's own slug becomes a link into its panel, which Home Assistant opens in place.
#[test]
fn the_add_on_slug_links_the_device_page_to_the_panel() {
    let body =
        r#"{"result":"ok","data":{"slug":"a0d7b954_kessel","name":"Kessel","version":"0.1.0"}}"#;
    let slug = hassio::parse_self_slug(body.as_bytes()).expect("a well-formed self info");
    assert_eq!(slug, "a0d7b954_kessel");
    assert_eq!(
        hassio::ingress_url(&slug),
        "homeassistant://hassio/ingress/a0d7b954_kessel"
    );
}

/// An answer without a slug is refused rather than producing a link to nowhere.
#[test]
fn a_self_info_without_a_slug_is_refused() {
    for body in [
        r#"{"result":"ok","data":{"slug":""}}"#,
        r#"{"result":"error","message":"nope"}"#,
        "not json",
    ] {
        assert!(
            hassio::parse_self_slug(body.as_bytes()).is_err(),
            "{body} was accepted"
        );
    }
}
