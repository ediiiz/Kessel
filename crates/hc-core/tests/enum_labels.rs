//! An HA `select` label is resolved against the appliance's declared values, not the whole
//! enumeration catalogue.
//!
//! `EnumType::members` is value-to-name from the `FeatureMapping`, which names the whole
//! enumeration; `declared_values` is what this appliance's `DeviceDescription` supports. Resolving
//! against `members` alone would send values the appliance never declared.
//!
//! The round-trip half runs against the scrubbed dishwasher fixture and a recording transport.

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

use std::path::PathBuf;

use common::RecordingTransport;
use hc_core::bridge::{SetError, enum_value_for_label, enum_wire_value, handle_set};
use hc_core::mapping::{DeviceId, DeviceMapping, Excludes};
use hc_core::write::GuardedWriter;
use hc_profile::{EnumType, Profile};

/// An enumeration the mapping names three members of while the description declares only two.
///
/// Synthetic: none of the captured fixtures narrows an enumeration, but the model allows it.
/// `EnumType` is `#[non_exhaustive]`, so it is built from `Default` and filled.
fn narrowed_enum() -> EnumType {
    let mut enum_type = EnumType::default();
    enum_type.enid = 516;
    enum_type.key = Some("BSH.Common.EnumType.PowerState".to_owned());
    enum_type.members.insert(1, "Off".to_owned());
    enum_type.members.insert(2, "On".to_owned());
    // Cataloged by the mapping, NOT declared by this appliance's description.
    enum_type.members.insert(3, "Standby".to_owned());
    enum_type.declared_values = vec![1, 2];
    enum_type
}

#[test]
fn undeclared_enum_label_is_refused() {
    let enum_type = narrowed_enum();

    // A declared member still resolves — the narrowing must not break the working case.
    assert_eq!(
        enum_value_for_label(&enum_type, "On"),
        Some(2),
        "a label the appliance declares must still resolve to its wire integer"
    );
    assert_eq!(enum_value_for_label(&enum_type, "Off"), Some(1));

    // The point of the test: cataloged but undeclared.
    assert_eq!(
        enum_value_for_label(&enum_type, "Standby"),
        None,
        "'Standby' is named by the FeatureMapping but is NOT in this appliance's declared_values — \
         resolving it would send a value the appliance never said it supports"
    );

    // A label in neither, for completeness.
    assert_eq!(enum_value_for_label(&enum_type, "Nonexistent"), None);
}

/// The ordinal arm resolves what the appliance declares, and only that.
#[test]
fn a_declared_ordinal_resolves_to_its_wire_value() {
    let enum_type = narrowed_enum();

    assert_eq!(
        enum_wire_value(&enum_type, "2"),
        Some(2),
        "the ordinal the appliance reports for a declared member must resolve to itself"
    );
    assert_eq!(enum_wire_value(&enum_type, "1"), Some(1));
    // Whitespace is what a retained MQTT payload or a hand-written curl body carries.
    assert_eq!(
        enum_wire_value(&enum_type, " 2 "),
        Some(2),
        "surrounding whitespace is not part of the value"
    );
    // Not an integer, not a label: neither arm can claim it.
    assert_eq!(enum_wire_value(&enum_type, "2.0"), None);
    assert_eq!(enum_wire_value(&enum_type, ""), None);
}

/// An ordinal the appliance never declared is refused locally. `"3"` is a well-formed integer, so
/// without the `declared_values` check it would reach the appliance.
#[test]
fn an_undeclared_ordinal_is_refused() {
    let enum_type = narrowed_enum();

    assert_eq!(
        enum_wire_value(&enum_type, "3"),
        None,
        "3 is named by the FeatureMapping but is NOT in this appliance's declared_values — \
         accepting the ordinal must not smuggle in what the label arm refuses"
    );
    // In neither the catalog nor the declaration.
    assert_eq!(enum_wire_value(&enum_type, "99"), None);
    assert_eq!(enum_wire_value(&enum_type, "-1"), None);
}

/// The label arm runs first: with member `7` named `"0"`, the payload `"0"` resolves to 7, not to
/// the value 0.
#[test]
fn a_label_still_wins_over_an_ordinal_reading() {
    let mut enum_type = narrowed_enum();
    enum_type.members.insert(7, "0".to_owned());
    enum_type.declared_values = vec![0, 1, 2, 7];

    assert_eq!(
        enum_wire_value(&enum_type, "0"),
        Some(7),
        "the label arm is tried first: '0' is a declared member NAME here, so it resolves to 7 — \
         reading it as the ordinal 0 would silently change what an existing call writes"
    );
    // And the ordinal arm still covers a value with no name at all.
    assert_eq!(
        enum_wire_value(&enum_type, "1"),
        Some(1),
        "an ordinal that is not any member's name resolves through the ordinal arm"
    );
}

/// The scrubbed dishwasher profile — a real `DeviceDescription`/`FeatureMapping` pair, no appliance.
fn dishwasher_fixture() -> Profile {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("hc-profile")
        .join("tests")
        .join("fixtures")
        .join("dishwasher-tls.zip");
    Profile::from_zip_bytes(
        &std::fs::read(&path).unwrap_or_else(|error| panic!("reading {}: {error}", path.display())),
    )
    .expect("the scrubbed dishwasher fixture parses")
}

#[tokio::test]
async fn an_unresolvable_label_never_reaches_the_transport() {
    let profile = dishwasher_fixture();
    let model = profile.model();
    let mapping = DeviceMapping::build(&DeviceId::new("test-device"), model, &Excludes::none());
    let power = mapping
        .entries()
        .iter()
        .find(|entry| entry.key.as_deref() == Some("BSH.Common.Setting.PowerState"))
        .expect("the dishwasher declares a writable PowerState");
    let set_topic = power
        .command_topic
        .clone()
        .expect("PowerState is a writable feature");

    let recorder = RecordingTransport::new();
    let mut writer = GuardedWriter::new(recorder.clone());
    let result = handle_set(
        &mut writer,
        &hc_core::bridge::SetContext::new(model, &mapping),
        &set_topic,
        "Standby",
    )
    .await;
    assert!(
        matches!(result, Err(SetError::EnumLabelUnknown(ref label)) if label == "Standby"),
        "a label this appliance does not declare must be refused locally: {result:?}"
    );
    assert_eq!(
        recorder.count(),
        0,
        "the refusal is local — nothing reached the transport"
    );
}

/// An appliance reports an enumeration as its wire ordinal (in the shadow and in `NOTIFY`), so
/// writing that ordinal straight back must reach the transport. The ordinal is taken from the
/// fixture's `declared_values` rather than written as a literal.
#[tokio::test]
async fn an_ordinal_the_appliance_reported_reaches_the_transport() {
    let profile = dishwasher_fixture();
    let model = profile.model();
    let mapping = DeviceMapping::build(&DeviceId::new("test-device"), model, &Excludes::none());
    let power = mapping
        .entries()
        .iter()
        .find(|entry| entry.key.as_deref() == Some("BSH.Common.Setting.PowerState"))
        .expect("the dishwasher declares a writable PowerState");
    let set_topic = power
        .command_topic
        .clone()
        .expect("PowerState is a writable feature");
    let entity = model
        .entity(power.uid)
        .expect("the mapping's uid is in the model");
    let enum_type = model
        .enum_type_of(entity)
        .expect("PowerState is an enumeration");
    let reported = *enum_type
        .declared_values
        .first()
        .expect("the appliance declares at least one PowerState value");

    let recorder = RecordingTransport::new();
    let mut writer = GuardedWriter::new(recorder.clone());
    let result = handle_set(
        &mut writer,
        &hc_core::bridge::SetContext::new(model, &mapping),
        &set_topic,
        &reported.to_string(),
    )
    .await;
    assert!(
        result.is_ok(),
        "an ordinal this appliance declares must resolve: {result:?}"
    );

    let sent = recorder.sent.lock().clone();
    assert_eq!(sent.len(), 1, "exactly one command reached the transport");
    let command = sent.first().expect("one command was just asserted");
    assert_eq!(
        command.target_uid,
        Some(power.uid),
        "the command targets the feature the set topic named"
    );
    assert_eq!(
        command.value,
        serde_json::Value::from(reported),
        "the ordinal is carried through as the wire integer, not as text"
    );
}

// ---------------------------------------------------------------------------------------------
// A contributed member name reaches Home Assistant, and comes back.
// ---------------------------------------------------------------------------------------------

/// An MQTT client that records what was published. `hc-core`'s own fake broker lives in
/// `tests/mqtt.rs`, which is a separate test binary.
#[derive(Debug, Default)]
struct CollectingClient {
    published: Vec<(String, Vec<u8>)>,
}

#[async_trait::async_trait]
impl hc_core::mqtt::MqttClient for CollectingClient {
    async fn publish(
        &mut self,
        topic: &str,
        payload: &[u8],
        _retain: bool,
    ) -> Result<(), hc_core::mqtt::MqttError> {
        self.published.push((topic.to_owned(), payload.to_vec()));
        Ok(())
    }

    async fn subscribe(&mut self, _topic_filter: &str) -> Result<(), hc_core::mqtt::MqttError> {
        Ok(())
    }
}

/// A catalogue that names two of `EcoAsDefault`'s members, and nothing else.
///
/// Stands in for `apps/server`'s contributed TOML, which `hc-core` cannot see. The labels match
/// `prefixes/Dishcare.Dishwasher.toml`.
#[derive(Debug, Clone, Copy)]
struct TestCatalogue;

const ECO_KEY: &str = "Dishcare.Dishwasher.Setting.EcoAsDefault";

impl hc_core::discovery::FeatureNaming for TestCatalogue {
    fn feature_name(&self, _key: &str) -> Option<String> {
        None
    }

    fn member_name(&self, key: &str, member: &str) -> Option<String> {
        match (key, member) {
            (ECO_KEY, "LastProgram") => Some("Whatever ran last".to_owned()),
            (ECO_KEY, "EcoAsDefault") => Some("Always Eco".to_owned()),
            _ => None,
        }
    }
}

/// The mapping entry for `EcoAsDefault`, a `readWrite` two-member enumeration.
fn eco_as_default(mapping: &DeviceMapping) -> &hc_core::mapping::EntityMapping {
    mapping
        .entries()
        .iter()
        .find(|entry| entry.key.as_deref() == Some(ECO_KEY))
        .expect("the dishwasher declares a writable EcoAsDefault")
}

/// `EcoAsDefault`'s announced `options`, through [`TestCatalogue`].
fn announced_options(device: &DeviceId, mapping: &DeviceMapping) -> serde_json::Value {
    let info = hc_core::discovery::DeviceInfo::default();
    let configs = hc_core::discovery::discovery_configs(
        "kessel",
        device,
        mapping,
        &hc_core::discovery::Announcement {
            mode: hc_core::discovery::DiscoveryMode::Full,
            info: &info,
            names: &TestCatalogue,
            origin: &hc_core::discovery::DiscoveryOrigin::KESSEL,
        },
    );
    configs
        .iter()
        .find(|config| config.id == "Dishcare_Dishwasher_Setting_EcoAsDefault")
        .expect("EcoAsDefault is announced")
        .payload
        .get("options")
        .cloned()
        .expect("a select announces its options")
}

#[tokio::test]
async fn a_contributed_member_name_is_offered_published_and_accepted_back() {
    // Discovery `options`, the state topic and `…/set` are asserted together: HA validates every
    // state against `options`, so a label missing from the state topic freezes the entity, and a
    // label sent back on `…/set` that maps to nothing is a dead control.
    let profile = dishwasher_fixture();
    let model = profile.model();
    let device = DeviceId::new("test-device");
    let mapping = DeviceMapping::build(&device, model, &Excludes::none());
    let entry = eco_as_default(&mapping);
    assert_eq!(
        entry.options,
        vec!["LastProgram".to_owned(), "EcoAsDefault".to_owned()],
        "premise: the appliance declares these two members, in this order"
    );

    // 1. `options`, at discovery.
    assert_eq!(
        announced_options(&device, &mapping),
        serde_json::json!(["Whatever ran last", "Always Eco"]),
        "the options HA renders are the contributed names, in the appliance's declared order"
    );

    // 2. The state topic, when the appliance reports the ordinal.
    let bridge = hc_core::bridge::DeviceBridge::new("kessel", device.clone())
        .with_names(std::sync::Arc::new(TestCatalogue));
    let mut broker = CollectingClient::default();
    bridge
        .publish_value(
            &mut broker,
            &mapping,
            model,
            &hc_proto::ValueUpdate {
                uid: u64::from(entry.uid),
                value: serde_json::Value::from(0),
                description: hc_proto::DescriptionUpdate::default(),
            },
        )
        .await
        .expect("publish");
    let published = broker
        .published
        .iter()
        .find(|(topic, _)| topic.contains("EcoAsDefault"))
        .expect("the value is published");
    assert_eq!(
        String::from_utf8_lossy(&published.1),
        "Whatever ran last",
        "the state must be one of the `options` HA was given, or HA discards it and the entity \
         stays on its last value"
    );

    // 3. Back in through `…/set`.
    let set_topic = entry
        .command_topic
        .clone()
        .expect("EcoAsDefault is writable");
    let recorder = RecordingTransport::new();
    let mut writer = GuardedWriter::new(recorder.clone());
    handle_set(
        &mut writer,
        &hc_core::bridge::SetContext::new(model, &mapping).with_names(&TestCatalogue),
        &set_topic,
        "Whatever ran last",
    )
    .await
    .expect("a contributed name HA was given must be writable back");
    let sent = recorder.sent.lock().clone();
    assert_eq!(
        sent.first().map(|command| command.value.clone()),
        Some(serde_json::Value::from(0)),
        "the appliance receives its own ordinal — a contributed name never reaches the wire"
    );
}

#[tokio::test]
async fn the_appliances_own_member_name_still_writes_after_labelling() {
    // The appliance's own member name, as sent by REST, `mosquitto_pub` or existing automations,
    // still works, and so does a bare ordinal.
    let profile = dishwasher_fixture();
    let model = profile.model();
    let mapping = DeviceMapping::build(&DeviceId::new("test-device"), model, &Excludes::none());
    let entry = eco_as_default(&mapping);
    let set_topic = entry.command_topic.clone().expect("writable");

    for payload in ["LastProgram", "0"] {
        let recorder = RecordingTransport::new();
        let mut writer = GuardedWriter::new(recorder.clone());
        handle_set(
            &mut writer,
            &hc_core::bridge::SetContext::new(model, &mapping).with_names(&TestCatalogue),
            &set_topic,
            payload,
        )
        .await
        .unwrap_or_else(|error| panic!("`{payload}` must still write: {error}"));
        assert_eq!(
            recorder.sent.lock().first().map(|c| c.value.clone()),
            Some(serde_json::Value::from(0)),
            "`{payload}` resolves to the appliance's own ordinal"
        );
    }
}

/// A catalogue whose label for one member **is another member's own name**.
#[derive(Debug, Clone, Copy)]
struct AmbiguousCatalogue;

impl hc_core::discovery::FeatureNaming for AmbiguousCatalogue {
    fn feature_name(&self, _key: &str) -> Option<String> {
        None
    }

    fn member_name(&self, key: &str, member: &str) -> Option<String> {
        // `LastProgram` is labelled with the OTHER member's raw name.
        (key == ECO_KEY && member == "LastProgram").then(|| "EcoAsDefault".to_owned())
    }
}

#[tokio::test]
async fn an_ambiguous_contributed_name_resolves_to_the_appliances_own_member() {
    // A contributed label that collides with another member's raw name could send the wrong value.
    // The appliance's own member name always wins: a label is only reversed for a payload that is
    // not already a declared member, so a contributed file cannot change what a payload means.
    let profile = dishwasher_fixture();
    let model = profile.model();
    let mapping = DeviceMapping::build(&DeviceId::new("test-device"), model, &Excludes::none());
    let entry = eco_as_default(&mapping);
    let set_topic = entry.command_topic.clone().expect("writable");

    let recorder = RecordingTransport::new();
    let mut writer = GuardedWriter::new(recorder.clone());
    handle_set(
        &mut writer,
        &hc_core::bridge::SetContext::new(model, &mapping).with_names(&AmbiguousCatalogue),
        &set_topic,
        "EcoAsDefault",
    )
    .await
    .expect("a declared member name is always writable");
    assert_eq!(
        recorder.sent.lock().first().map(|c| c.value.clone()),
        Some(serde_json::Value::from(1)),
        "`EcoAsDefault` is the appliance's own member 1 — a contributed label pointing elsewhere \
         must not redirect it to 0"
    );
}
