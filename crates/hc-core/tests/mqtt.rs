//! MQTT publish/subscribe and Home Assistant discovery, against an in-process fake broker and a
//! fake write transport; no real broker or device.
//!
//! The discovery tests run over all four scrubbed descriptions; the write and identifier tests use
//! the ones whose features exercise the property.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;

use hc_core::bridge::{DeviceBridge, handle_set};
use hc_core::discovery::{Announcement, DiscoveryMode};
use hc_core::mapping::{DeviceId, DeviceMapping, EntityClass, EntityMapping, Excludes};
use hc_core::mqtt::{LastWill, MqttClient, MqttError};
use hc_core::write::{GuardedWriter, WriteCommand, WriteTransport};
use hc_profile::{EntityModel, Profile};

/// The four scrubbed fixtures in `hc-profile`'s test tree, read-only.
const FIXTURES: [&str; 4] = [
    "hob-aes.zip",
    "fridge-tls.zip",
    "dishwasher-tls.zip",
    "oven-tls.zip",
];

/// The topic root every test publishes under.
const BASE: &str = "kessel";

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("hc-profile")
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn fixture_profile(name: &str) -> Profile {
    let bytes = std::fs::read(fixture_path(name))
        .unwrap_or_else(|err| panic!("reading fixture {name}: {err}"));
    Profile::from_zip_bytes(&bytes).unwrap_or_else(|err| panic!("parsing fixture {name}: {err}"))
}

/// A synthetic operator device slug (never a real haId/serial/mac).
fn device_id() -> DeviceId {
    DeviceId::new("test-device")
}

/// Whether a kind of node carries a value, enumerated independently of `hc_core` so the test is
/// not tautological.
///
/// `access` is not consulted: it is a permission claim, not a statement about what arrives. The
/// fridge declares `access="none"` on its settings and still reports values for them.
const fn is_value_bearing(kind: hc_profile::EntityKind) -> bool {
    use hc_profile::EntityKind as Kind;
    matches!(
        kind,
        Kind::Status
            | Kind::Setting
            | Kind::Event
            | Kind::Option
            | Kind::ActiveProgram
            | Kind::SelectedProgram
    )
}

/// How many entities the source model reports a value for, counted independently of the mapping.
fn value_bearing_count(model: &EntityModel) -> usize {
    model
        .entities()
        .values()
        .filter(|entity| is_value_bearing(entity.kind))
        .count()
}

/// Whether a kind of node can be an entity at all: value-bearing, or a `<command>` button.
/// Containers, `<program>` and the hob's `<protectionPort>` are structure, not surface.
const fn is_addressable(kind: hc_profile::EntityKind) -> bool {
    is_value_bearing(kind) || matches!(kind, hc_profile::EntityKind::Command)
}

/// Whether a feature takes a `…/set` topic, stated independently of the mapping.
///
/// Writable means `readWrite`/`writeOnly` or `access="none"`; `read`/`readStatic`, an absent
/// attribute and an unrecognised word stay out (see `hc_core::write::check_access`).
fn takes_a_set_topic(entity: &hc_profile::Entity) -> bool {
    match entity.access {
        Some(hc_profile::Access::None) => is_addressable(entity.kind),
        ref access => access.as_ref().is_some_and(hc_profile::Access::is_writable),
    }
}

/// The number of surfaceable features in the source model, those that get a discovery config:
/// addressable, and carrying a value or a write. A `<command>` qualifies through
/// `takes_a_set_topic`.
fn surfaceable_count(model: &EntityModel) -> usize {
    model
        .entities()
        .values()
        .filter(|entity| {
            is_addressable(entity.kind)
                && (is_value_bearing(entity.kind) || takes_a_set_topic(entity))
        })
        .count()
}

/// An in-process fake MQTT broker: records every publish and subscription, and holds the
/// connect-time will.
#[derive(Debug, Default)]
struct FakeBroker {
    publishes: Vec<Published>,
    subscriptions: Vec<String>,
    last_will: Option<LastWill>,
}

/// One recorded publish.
#[derive(Debug, Clone)]
struct Published {
    topic: String,
    payload: Vec<u8>,
    retain: bool,
}

impl FakeBroker {
    fn new() -> Self {
        Self::default()
    }

    /// A broker whose connect-time will is `will` (LWT modelling).
    fn with_will(will: LastWill) -> Self {
        Self {
            last_will: Some(will),
            ..Self::default()
        }
    }

    /// Every entity config, as Home Assistant reads it out of the device messages: each component
    /// with the shared root keys merged in, under the per-entity topic it would have had
    /// (`homeassistant/<platform>/<slug>/<id>/config`). `platform` and `origin` are left out, as
    /// HA does.
    fn discovery_publishes(&self) -> std::vec::IntoIter<Published> {
        self.device_messages()
            .flat_map(|(slug, message, retain)| expand_device_message(&slug, &message, retain))
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// The device discovery messages published, decoded: `(slug, message, retained)`.
    fn device_messages(&self) -> impl Iterator<Item = (String, Value, bool)> + '_ {
        self.publishes.iter().filter_map(|published| {
            let slug = published
                .topic
                .strip_prefix("homeassistant/device/kessel_")?
                .strip_suffix("/config")?
                .to_owned();
            let message = serde_json::from_slice(&published.payload).ok()?;
            Some((slug, message, published.retain))
        })
    }

    /// Publishes to a `…/state` topic under the device root.
    fn state_publishes(&self) -> impl Iterator<Item = &Published> {
        self.publishes
            .iter()
            .filter(|p| p.topic.ends_with("/state"))
    }
}

/// One device message's components as standalone entity configs; see
/// [`FakeBroker::discovery_publishes`].
fn expand_device_message(slug: &str, message: &Value, retain: bool) -> Vec<Published> {
    let Some(root) = message.as_object() else {
        return Vec::new();
    };
    let shared: serde_json::Map<String, Value> = root
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "components" | "origin"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let components = root.get("components").and_then(Value::as_object);
    components
        .into_iter()
        .flatten()
        .filter_map(|(id, component)| {
            let component = component.as_object()?;
            // A platform-only stub is a retraction, not a config.
            if component.len() == 1 {
                return None;
            }
            let mut merged = shared.clone();
            merged.extend(component.clone());
            let platform = merged.remove("platform")?;
            Some(Published {
                topic: format!("homeassistant/{}/{slug}/{id}/config", platform.as_str()?),
                payload: serde_json::to_vec(&Value::Object(merged)).ok()?,
                retain,
            })
        })
        .collect()
}

#[async_trait]
impl MqttClient for FakeBroker {
    async fn publish(
        &mut self,
        topic: &str,
        payload: &[u8],
        retain: bool,
    ) -> Result<(), MqttError> {
        self.publishes.push(Published {
            topic: topic.to_owned(),
            payload: payload.to_vec(),
            retain,
        });
        Ok(())
    }

    async fn subscribe(&mut self, topic_filter: &str) -> Result<(), MqttError> {
        self.subscriptions.push(topic_filter.to_owned());
        Ok(())
    }
}

#[tokio::test]
async fn full_publishes_every_feature() {
    // `full` publishes one discovery config per surfaceable feature, counted independently in the
    // source model.
    for name in FIXTURES {
        let profile = fixture_profile(name);
        let model = profile.model();
        let expected = surfaceable_count(model);
        assert!(
            expected > 0,
            "{name}: fixture should have surfaceable features"
        );

        let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());
        let bridge = DeviceBridge::new(BASE, device_id());
        let mut broker = FakeBroker::new();

        bridge
            .publish_discovery(
                &mut broker,
                &mapping,
                &Announcement::bare(DiscoveryMode::Full),
            )
            .await
            .expect("publish discovery");

        // Plus the synthetic controls, which back no feature. Counted from the same predicate
        // rather than hardcoded.
        let synthetic = hc_core::discovery::synthetic_control_topics(&mapping).len();
        let produced = broker.discovery_publishes().count();
        assert_eq!(
            produced,
            expected + synthetic,
            "{name}: `full` must publish one discovery config per surfaceable feature, plus the \
             {synthetic} synthetic control(s) (produced {produced}, model has {expected} surfaceable)"
        );
        assert!(
            broker.discovery_publishes().all(|p| p.retain),
            "{name}: discovery configs must be retained so HA sees them on (re)subscribe"
        );
    }
}

/// The set of `…/state` topics a device's mapping covers, fully qualified.
///
/// Coverage lives in the mapping: a state topic is only published once there is a value for it.
fn mapped_state_topics(mapping: &DeviceMapping, device: &DeviceId) -> BTreeSet<String> {
    mapping
        .state_topics()
        .map(|relative| format!("{BASE}/{}/{relative}", device.as_str()))
        .collect()
}

#[tokio::test]
async fn curated_narrows_discovery_but_not_state_topics() {
    // `curated` trims the discovery surface but not coverage: fewer discovery configs than `full`,
    // the same state-topic set, equal to the readable-feature count.
    for name in FIXTURES {
        let profile = fixture_profile(name);
        let model = profile.model();
        let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());
        let bridge = DeviceBridge::new(BASE, device_id());

        let mut full = FakeBroker::new();
        bridge
            .announce(
                &mut full,
                &mapping,
                &Announcement::bare(DiscoveryMode::Full),
            )
            .await
            .expect("announce full");

        let mut curated = FakeBroker::new();
        bridge
            .announce(
                &mut curated,
                &mapping,
                &Announcement::bare(DiscoveryMode::Curated),
            )
            .await
            .expect("announce curated");

        let full_discovery = full.discovery_publishes().count();
        let curated_discovery = curated.discovery_publishes().count();
        assert!(
            curated_discovery < full_discovery,
            "{name}: curated must narrow the discovery surface \
             (curated {curated_discovery} !< full {full_discovery})"
        );

        // `DiscoveryMode` is not an input to the mapping, so the covered set must be the whole
        // value-bearing surface of the source model.
        let covered = mapped_state_topics(&mapping, &device_id());
        assert_eq!(
            covered.len(),
            value_bearing_count(model),
            "{name}: every value-bearing feature must be covered by a state topic"
        );
        assert!(!covered.is_empty(), "{name}: no state topic is covered");

        // The `…/set` subscriptions are mode-independent: curation must not unsubscribe a writable
        // feature.
        assert_eq!(
            full.subscriptions, curated.subscriptions,
            "{name}: curation must NOT change the `…/set` subscription coverage"
        );
    }
}

#[tokio::test]
async fn an_announce_publishes_no_state_it_does_not_have() {
    // An announce must not publish an empty retained payload to a state topic. HA logs an invalid
    // option for every enum entity, and the empty retained message overwrites the last real value
    // until the appliance next reports. A state topic is published only when there is a value.
    for name in FIXTURES {
        let profile = fixture_profile(name);
        let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
        let bridge = DeviceBridge::new(BASE, device_id());
        let mut broker = FakeBroker::new();

        bridge
            .announce(
                &mut broker,
                &mapping,
                &Announcement::bare(DiscoveryMode::Full),
            )
            .await
            .expect("announce");

        // Scoped to the device's own topic root: under `homeassistant/`, an empty retained payload
        // is how a stale config is deleted, which other tests require.
        let root = format!("{BASE}/{}/", device_id().as_str());
        let empty: Vec<&str> = broker
            .publishes
            .iter()
            .filter(|published| published.payload.is_empty() && published.topic.starts_with(&root))
            .map(|published| published.topic.as_str())
            .collect();
        assert!(
            empty.is_empty(),
            "{name}: an announce published {} empty retained payload(s) under {root} — HA rejects \
             '' against an entity's options, and a retained '' erases the previous run's value. \
             First: {:?}",
            empty.len(),
            empty.first()
        );
    }
}

#[tokio::test]
async fn a_sensor_config_offers_no_command_topic() {
    // `command_topic` is advertised only on the platforms that write (switch, select, number). The
    // raw `…/set` topic is still subscribed for every writable feature.
    for name in FIXTURES {
        let profile = fixture_profile(name);
        let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
        let bridge = DeviceBridge::new(BASE, device_id());
        let mut broker = FakeBroker::new();

        bridge
            .publish_discovery(
                &mut broker,
                &mapping,
                &Announcement::bare(DiscoveryMode::Full),
            )
            .await
            .expect("publish discovery");

        let stateful = broker.discovery_publishes().filter(|published| {
            !matches!(
                component_of_topic(&published.topic),
                "switch" | "select" | "number" | "button"
            )
        });

        for published in stateful {
            let component = component_of_topic(&published.topic);
            let payload: Value =
                serde_json::from_slice(&published.payload).expect("a discovery payload is JSON");
            assert_eq!(
                payload.get("command_topic"),
                None,
                "{name}: {} is a `{component}` and must advertise no command topic",
                published.topic
            );
        }
    }
}

/// The `device` object of a discovery payload, or a panic naming the config that lacks one.
fn device_block_of<'p>(payload: &'p Value, fixture: &str, topic: &str) -> &'p Value {
    payload.get("device").unwrap_or_else(|| {
        panic!(
            "{fixture}: {topic} has no `device` block, so HA creates an entity with no device: \
             {payload}"
        )
    })
}

#[tokio::test]
async fn every_discovery_config_carries_a_device_block() {
    // Without a `device` object HA registers each config as a standalone entity and creates no
    // device. The identity is built from the caller's slug only; no appliance identifier reaches
    // it (`no_identifier_in_topics_or_discovery` sweeps the whole payload).
    for name in FIXTURES {
        let profile = fixture_profile(name);
        let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
        let bridge = DeviceBridge::new(BASE, device_id());
        let mut broker = FakeBroker::new();

        bridge
            .publish_discovery(
                &mut broker,
                &mapping,
                &Announcement::bare(DiscoveryMode::Full),
            )
            .await
            .expect("publish discovery");

        let mut checked = 0_usize;
        for published in broker.discovery_publishes() {
            let payload: Value =
                serde_json::from_slice(&published.payload).expect("a discovery payload is JSON");
            let device = device_block_of(&payload, name, &published.topic);
            assert_eq!(
                device.get("identifiers"),
                Some(&Value::Array(vec![Value::String(format!(
                    "kessel_{}",
                    device_id().as_str()
                ))])),
                "{name}: the device identity is the slug under a Kessel-scoped prefix — one device \
                 per appliance, and no collision with another integration's bare slug"
            );
            assert_eq!(
                device.get("name"),
                Some(&Value::String(device_id().as_str().to_owned())),
                "{name}: the device name is the operator's slug, the only name this crate may see"
            );
            checked += 1;
        }
        assert!(checked > 0, "{name}: no discovery config was published");
    }
}

#[tokio::test]
async fn the_device_block_names_the_brand_and_the_model_it_was_given() {
    // HA shows `manufacturer` and `model` under the device name, which is how an operator tells the
    // appliances apart. Brand and model are not secret. The identity is the caller's to supply;
    // this crate reads no profile.
    let info = hc_core::discovery::DeviceInfo {
        configuration_url: None,
        brand: Some("BOSCH".to_owned()),
        model: Some("HRG7784B1".to_owned()),
        sw_version: Some("1.4".to_owned()),
    };
    for name in FIXTURES {
        let profile = fixture_profile(name);
        let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
        let bridge = DeviceBridge::new(BASE, device_id());
        let mut broker = FakeBroker::new();

        bridge
            .publish_discovery(
                &mut broker,
                &mapping,
                &Announcement {
                    mode: DiscoveryMode::Full,
                    info: &info,
                    names: &hc_core::discovery::DerivedNames,
                    origin: &hc_core::discovery::DiscoveryOrigin::KESSEL,
                },
            )
            .await
            .expect("publish discovery");

        let mut checked = 0_usize;
        for published in broker.discovery_publishes() {
            let payload: Value =
                serde_json::from_slice(&published.payload).expect("a discovery payload is JSON");
            let device = device_block_of(&payload, name, &published.topic);
            assert_eq!(
                device.get("manufacturer"),
                Some(&Value::String("BOSCH".to_owned())),
                "{name}: {} names no manufacturer, so HA's device page shows no maker",
                published.topic
            );
            assert_eq!(
                device.get("model"),
                Some(&Value::String("HRG7784B1".to_owned())),
                "{name}: {} names no model, so HA's device page shows no model designation",
                published.topic
            );
            assert_eq!(
                device.get("sw_version"),
                Some(&Value::String("1.4".to_owned())),
                "{name}: {} carries no sw_version",
                published.topic
            );
            checked += 1;
        }
        assert!(checked > 0, "{name}: no discovery config was published");
    }
}

#[tokio::test]
async fn an_unknown_brand_or_model_is_omitted_rather_than_filled_in() {
    // `DeviceInfo::default()` knows nothing, so the fields must be absent from the payload rather
    // than filled with a placeholder HA would display verbatim.
    let profile = fixture_profile("oven-tls.zip");
    let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
    let bridge = DeviceBridge::new(BASE, device_id());
    let mut broker = FakeBroker::new();

    bridge
        .publish_discovery(
            &mut broker,
            &mapping,
            &Announcement::bare(DiscoveryMode::Full),
        )
        .await
        .expect("publish discovery");

    for published in broker.discovery_publishes() {
        let payload: Value =
            serde_json::from_slice(&published.payload).expect("a discovery payload is JSON");
        let device = device_block_of(&payload, "oven-tls.zip", &published.topic);
        for key in ["manufacturer", "model", "sw_version"] {
            assert_eq!(
                device.get(key),
                None,
                "{}: `{key}` is absent from the identity it was given, so it must be absent from \
                 the payload rather than filled with a placeholder",
                published.topic
            );
        }
    }
}

/// A [`hc_core::discovery::FeatureNaming`] backed by a fixed table, shaped like the server's label
/// catalogue.
#[derive(Debug)]
struct TableNames(std::collections::BTreeMap<&'static str, &'static str>);

impl hc_core::discovery::FeatureNaming for TableNames {
    fn feature_name(&self, key: &str) -> Option<String> {
        self.0.get(key).map(|name| (*name).to_owned())
    }
}

/// The discovery payload for the config whose topic ends `/<object_id>/config`.
fn config_named(broker: &FakeBroker, object_id: &str) -> (String, Value) {
    let suffix = format!("/{object_id}/config");
    let published = broker
        .discovery_publishes()
        .find(|published| published.topic.ends_with(&suffix))
        .unwrap_or_else(|| panic!("no discovery config for object id `{object_id}`"));
    let payload = serde_json::from_slice(&published.payload).expect("a discovery payload is JSON");
    (published.topic, payload)
}

#[tokio::test]
async fn an_entity_is_named_for_its_leaf_not_its_whole_dotted_key() {
    // `name` is the leaf of the feature key, so HA has something short to render. `default_entity_id`
    // is the whole key, slugified and device-scoped, so the entity id stays unique and stable even
    // if the name is later corrected or translated.
    let profile = fixture_profile("oven-tls.zip");
    let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
    let bridge = DeviceBridge::new(BASE, device_id());
    let mut broker = FakeBroker::new();

    bridge
        .publish_discovery(
            &mut broker,
            &mapping,
            &Announcement::bare(DiscoveryMode::Full),
        )
        .await
        .expect("publish discovery");

    let (topic, payload) = config_named(&broker, "BSH_Common_Status_OperationState");
    assert_eq!(
        payload.get("name"),
        Some(&Value::String("OperationState".to_owned())),
        "{topic}: the name must be the feature's leaf, not its whole dotted key"
    );
    // Lower-cased with every non-alphanumeric run collapsed to one `_`, because HA would otherwise
    // rewrite the id itself. The `sensor.` prefix is part of the value.
    assert_eq!(
        payload.get("default_entity_id"),
        Some(&Value::String(
            "sensor.test_device_bsh_common_status_operationstate".to_owned()
        )),
        "{topic}: the entity id must be the whole key, device-scoped and slugified, so it stays \
         unique and independent of the name"
    );
}

#[tokio::test]
async fn a_contributed_name_overrides_the_leaf_but_never_the_entity_id() {
    // A catalogue supplies the name, but `default_entity_id` and `unique_id` are untouched by it:
    // otherwise correcting a label would rename the entity in every HA install and break
    // automations, dashboards and history.
    let profile = fixture_profile("oven-tls.zip");
    let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
    let bridge = DeviceBridge::new(BASE, device_id());
    let mut broker = FakeBroker::new();
    let names = TableNames(
        std::iter::once(("BSH.Common.Status.OperationState", "Betriebszustand")).collect(),
    );

    bridge
        .publish_discovery(
            &mut broker,
            &mapping,
            &Announcement {
                mode: DiscoveryMode::Full,
                info: &hc_core::discovery::DeviceInfo::default(),
                names: &names,
                origin: &hc_core::discovery::DiscoveryOrigin::KESSEL,
            },
        )
        .await
        .expect("publish discovery");

    let (topic, payload) = config_named(&broker, "BSH_Common_Status_OperationState");
    assert_eq!(
        payload.get("name"),
        Some(&Value::String("Betriebszustand".to_owned())),
        "{topic}: a contributed name must reach `name`"
    );
    assert_eq!(
        payload.get("default_entity_id"),
        Some(&Value::String(
            "sensor.test_device_bsh_common_status_operationstate".to_owned()
        )),
        "{topic}: a contributed name must NOT reach the entity id — a corrected label would \
         otherwise rename the entity and break every automation referencing it"
    );
    assert_eq!(
        payload.get("unique_id"),
        Some(&Value::String(
            "test-device-BSH.Common.Status.OperationState".to_owned()
        )),
        "{topic}: nor the unique_id, which is what HA keys the entity registry on"
    );
}

/// Every discovery config the whole fleet produces, decoded: `(fixture, topic, payload)`.
async fn fleet_configs() -> Vec<(&'static str, String, Value)> {
    let mut configs = Vec::new();
    for fixture in FIXTURES {
        let broker = discovery_broker(fixture).await;
        for published in broker.discovery_publishes() {
            let payload =
                serde_json::from_slice(&published.payload).expect("a discovery payload is JSON");
            configs.push((fixture, published.topic.clone(), payload));
        }
    }
    configs
}

/// The uid a config topic belongs to, resolved through the mapping that produced it.
fn uid_of_config(mapping: &DeviceMapping, topic: &str) -> Option<u16> {
    // `homeassistant/<component>/<device>/<slot>/config`, where the slot is the feature key with
    // `.`→`_`.
    let slot = topic.split('/').nth(3)?;
    mapping
        .entries()
        .iter()
        .find(|entry| {
            entry
                .key
                .as_deref()
                .map(|key| key.replace('.', "_"))
                .as_deref()
                == Some(slot)
        })
        .map(|entry| entry.uid)
}

#[tokio::test]
async fn a_container_node_is_never_a_home_assistant_entity() {
    // Containers (`<settingList>`, `<statusList>`, …) hold no value of their own, so they get no
    // discovery config even when they declare `readWrite` (the fridge's light setting list does,
    // while the setting inside it declares `access="none"`). They keep their topics.
    for fixture in FIXTURES {
        let profile = fixture_profile(fixture);
        let model = profile.model();
        let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());
        let broker = discovery_broker(fixture).await;
        for published in broker.discovery_publishes() {
            assert_config_is_not_a_container(fixture, model, &mapping, &published.topic);
        }
    }
}

/// Asserts one config topic does not name a container node.
fn assert_config_is_not_a_container(
    fixture: &str,
    model: &EntityModel,
    mapping: &DeviceMapping,
    topic: &str,
) {
    // A synthetic control (`_control/stop_program`) has no uid and no entity behind it.
    let Some(uid) = uid_of_config(mapping, topic) else {
        return;
    };
    let kind = model
        .entity(uid)
        .expect("a mapped uid is in the model")
        .kind;
    assert!(
        !kind.is_container(),
        "{fixture}: {topic} announces {kind:?} {uid:#06X} as an entity — a container has no value \
         to report, so HA gets a sensor that reads `unknown` for ever"
    );
}

#[tokio::test]
async fn every_select_carries_options_even_when_there_are_none() {
    // HA's `select` schema requires `options`, and a config missing a required key is discarded
    // whole. A writable `selectedProgram` on an appliance with no programmes must therefore still
    // publish `"options": []`. A `sensor` with no members omits the key.
    let mut selects = 0_usize;
    for (fixture, topic, payload) in fleet_configs().await {
        if !topic.starts_with("homeassistant/select/") {
            continue;
        }
        selects += 1;
        assert!(
            payload.get("options").is_some_and(Value::is_array),
            "{fixture}: {topic}: a `select` without `options` is discarded whole by HA — the \
             entity never appears and nothing says why"
        );
    }
    assert!(
        selects > 0,
        "no select was published, so this proved nothing"
    );
}

#[tokio::test]
async fn an_announce_subscribes_home_assistants_birth_topic() {
    // Discovery configs are retained, but HA installed or reconfigured after the last announce has
    // nothing to re-read, and HA's `birth_message` declares its discovery cache cleared. So the
    // bridge re-announces on HA's birth message.
    let profile = fixture_profile("fridge-tls.zip");
    let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
    let bridge = DeviceBridge::new(BASE, device_id());
    let mut broker = FakeBroker::new();
    bridge
        .announce(
            &mut broker,
            &mapping,
            &Announcement::bare(DiscoveryMode::Full),
        )
        .await
        .expect("announce");

    assert!(
        broker
            .subscriptions
            .iter()
            .any(|topic| topic == hc_core::bridge::HA_STATUS_TOPIC),
        "an announce must subscribe {} — otherwise a Home Assistant that restarts with an empty \
         discovery cache stays empty until someone restarts Kessel. Subscribed: {} topic(s), none \
         of them it",
        hc_core::bridge::HA_STATUS_TOPIC,
        broker.subscriptions.len()
    );
}

#[test]
fn only_home_assistants_online_birth_message_is_a_re_announce() {
    // The same topic carries HA's will (`offline`); re-announcing to an HA that just died would be
    // wasted. Matched exactly on topic and payload.
    use hc_core::bridge::{HA_STATUS_TOPIC, is_home_assistant_birth};

    assert!(is_home_assistant_birth(HA_STATUS_TOPIC, b"online"));
    assert!(
        !is_home_assistant_birth(HA_STATUS_TOPIC, b"offline"),
        "HA's will is `offline` on the same topic"
    );
    assert!(
        !is_home_assistant_birth(HA_STATUS_TOPIC, b""),
        "a retained-clear on the status topic says nothing about HA being up"
    );
    assert!(
        !is_home_assistant_birth("homeassistant/status/extra", b"online"),
        "segment-exact, so a longer topic under the same prefix is not ours"
    );
    assert!(!is_home_assistant_birth(
        "kessel/fridge/availability",
        b"online"
    ));
}

/// A writable numeric feature is a `number` even when the appliance states no usable bounds.
///
/// The fridge declares `min="0" max="0" stepSize="0"` and `access="none"` on its setpoints, yet a
/// write reaches the device. Without stated bounds the config carries a non-binding range and
/// `mode: "box"`. Asserts the config is a `number` and its range admits the reported value (`-18`).
#[tokio::test]
async fn a_writable_number_the_appliance_would_not_bound_is_still_a_control() {
    // The fridge's freezer setpoint: `access="none" available="false" min="0" max="0" stepSize="0"`,
    // and the appliance reports -18 into it.
    const SLOT: &str = "Refrigeration_Common_Setting_Freezer_SetpointTemperature";
    const REPORTED: f64 = -18.0_f64;

    let broker = discovery_broker("fridge-tls.zip").await;
    let mut matched = broker
        .discovery_publishes()
        .filter(|published| published.topic.contains(&format!("/{SLOT}/config")));
    let published = matched
        .next()
        .expect("premise: a config is published for the freezer setpoint");
    assert!(
        matched.next().is_none(),
        "premise: exactly one, or the slot is being announced on two platforms at once"
    );
    let topic = published.topic.clone();
    let payload: Value =
        serde_json::from_slice(&published.payload).expect("a discovery payload is JSON");

    assert!(
        topic.starts_with("homeassistant/number/"),
        "a writable numeric feature is a control an operator can set, and whether the write \
         succeeds is the appliance's answer to give — got `{topic}`"
    );
    let min = payload
        .get("min")
        .and_then(Value::as_f64)
        .expect("a number carries a min");
    let max = payload
        .get("max")
        .and_then(Value::as_f64)
        .expect("a number carries a max");
    assert!(
        min <= REPORTED && REPORTED <= max,
        "the range must admit the value the appliance reports, or the control is the `[0, 0]` bug \
         again with different numbers: {REPORTED} is outside [{min}, {max}]"
    );
    assert_eq!(
        payload.get("mode").and_then(Value::as_str),
        Some("box"),
        "nobody stated a granularity for this range, so it is typed rather than dragged"
    );
}

#[tokio::test]
async fn no_config_carries_a_value_its_own_platform_rejects() {
    // A key that is present and permitted can still carry a value HA refuses, which discards the
    // entity just as completely. For example, HA's `number` schema requires a positive `step`, and
    // the fridge declares `stepSize="0"` on its setpoints.
    for (fixture, topic, payload) in fleet_configs().await {
        let Some(min) = payload.get("min").and_then(Value::as_f64) else {
            continue;
        };
        let max = payload
            .get("max")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("{fixture}: {topic}: `min` without `max`"));
        assert!(
            min < max,
            "{fixture}: {topic}: a `number` needs a range something can be set to — HA discards \
             `min` {min} / `max` {max}, and the appliance's reported value is outside it anyway"
        );
        let step = payload
            .get("step")
            .and_then(Value::as_f64)
            .unwrap_or(1.0_f64);
        assert!(
            step > 0.0_f64,
            "{fixture}: {topic}: HA's number schema requires a positive `step`; `{step}` fails \
             validation and the whole config is discarded"
        );
    }
}

#[tokio::test]
async fn an_announce_is_one_retained_device_message() {
    for fixture in FIXTURES {
        let broker = discovery_broker(fixture).await;
        let ha: Vec<&Published> = broker
            .publishes
            .iter()
            .filter(|published| published.topic.starts_with("homeassistant/"))
            .collect();
        assert_eq!(
            ha.len(),
            1,
            "{fixture}: one message per appliance, not one per entity"
        );
        let Some(message) = ha.first() else { continue };
        assert_eq!(
            message.topic,
            "homeassistant/device/kessel_test-device/config"
        );
        assert!(
            message.retain,
            "{fixture}: HA reads discovery from the retained copy"
        );

        let payload: Value = serde_json::from_slice(&message.payload).expect("JSON");
        // HA refuses a device message without either.
        let text = |pointer: &str| payload.pointer(pointer).and_then(Value::as_str);
        assert_eq!(text("/device/identifiers/0"), Some("kessel_test-device"));
        assert_eq!(text("/origin/name"), Some("Kessel"));
        assert_eq!(
            text("/availability_topic"),
            Some("kessel/test-device/availability")
        );
        let components = payload
            .get("components")
            .and_then(Value::as_object)
            .expect("components");
        assert!(!components.is_empty(), "{fixture}: no components");
        for (id, component) in components {
            let has = |key: &str| component.get(key).is_some_and(Value::is_string);
            assert!(
                has("platform") && has("unique_id"),
                "{fixture}: {id} needs a platform and a unique_id"
            );
            assert!(
                component.get("device").is_none() && component.get("availability_topic").is_none(),
                "{fixture}: {id} repeats a key the root already shares"
            );
        }
    }
}

#[tokio::test]
async fn the_origin_names_the_version_and_where_to_get_help() {
    let profile = fixture_profile("dishwasher-tls.zip");
    let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
    let origin = hc_core::discovery::DiscoveryOrigin {
        name: "Kessel",
        sw_version: Some("1.2.3"),
        support_url: Some("https://example.invalid/kessel"),
    };
    let message = hc_core::discovery::device_discovery(
        BASE,
        &device_id(),
        &mapping,
        &Announcement {
            origin: &origin,
            ..Announcement::bare(DiscoveryMode::Full)
        },
    );
    assert_eq!(
        message.payload.get("origin"),
        Some(&serde_json::json!({
            "name": "Kessel",
            "sw_version": "1.2.3",
            "support_url": "https://example.invalid/kessel",
        }))
    );
}

#[tokio::test]
async fn a_removed_component_is_retracted_as_a_platform_only_stub() {
    let profile = fixture_profile("dishwasher-tls.zip");
    let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
    let message = hc_core::discovery::device_discovery(
        BASE,
        &device_id(),
        &mapping,
        &Announcement::bare(DiscoveryMode::Full),
    );
    let platforms = message.platforms();
    assert_eq!(
        platforms
            .get("Dishcare_Dishwasher_Setting_TimeLight")
            .map(String::as_str),
        Some("select")
    );

    let gone = std::collections::BTreeMap::from([("Old_Feature".to_owned(), "switch".to_owned())]);
    let retraction = message.retracting(&gone);
    assert_eq!(
        retraction.pointer("/components/Old_Feature"),
        Some(&serde_json::json!({ "platform": "switch" }))
    );
    // Everything else is still announced in the same message.
    assert_eq!(
        retraction
            .get("components")
            .and_then(Value::as_object)
            .map(serde_json::Map::len),
        Some(platforms.len() + 1)
    );
}

#[test]
fn a_per_entity_config_from_an_earlier_version_is_recognised_for_migration() {
    use hc_core::discovery::{is_legacy_config, legacy_config_filter};
    let device = DeviceId::new("dishwasher");
    assert_eq!(
        legacy_config_filter(&device),
        "homeassistant/+/dishwasher/+/config"
    );

    let topic = "homeassistant/sensor/dishwasher/BSH_Common_Status_DoorState/config";
    let ours = br#"{"unique_id":"dishwasher-a","device":{"identifiers":["kessel_dishwasher"]}}"#;
    assert!(is_legacy_config(&device, topic, ours));
    // Already cleared, or someone else's device under the same node id.
    assert!(!is_legacy_config(&device, topic, b""));
    assert!(!is_legacy_config(
        &device,
        topic,
        br#"{"device":{"identifiers":["zigbee_dishwasher"]}}"#
    ));
    // The device message itself is not legacy, nor is another slug's config.
    assert!(!is_legacy_config(
        &device,
        "homeassistant/device/kessel_dishwasher/config",
        ours
    ));
    assert!(!is_legacy_config(
        &device,
        "homeassistant/sensor/oven/BSH_Common_Status_DoorState/config",
        ours
    ));
}

/// The keys Home Assistant requires of each platform, over and above `unique_id`; a missing one
/// discards the entity. Conservative: `switch` is absent because HA accepts an optimistic switch
/// with neither topic.
const REQUIRED_KEYS: [(&str, &[&str]); 5] = [
    ("sensor", &["state_topic"]),
    ("binary_sensor", &["state_topic"]),
    ("select", &["command_topic", "options"]),
    ("number", &["command_topic"]),
    ("button", &["command_topic"]),
];

#[tokio::test]
async fn no_config_omits_a_key_its_own_platform_requires() {
    let mut checked = 0_usize;
    for (fixture, topic, payload) in fleet_configs().await {
        let component = topic
            .split('/')
            .nth(1)
            .expect("a discovery topic has a component");
        assert!(
            payload.get("unique_id").is_some_and(Value::is_string),
            "{fixture}: {topic}: without a `unique_id` HA will not put the entity in its registry"
        );
        let Some((_, required)) = REQUIRED_KEYS.iter().find(|(name, _)| *name == component) else {
            continue;
        };
        checked += 1;
        for key in *required {
            assert!(
                payload.get(*key).is_some(),
                "{fixture}: {topic}: a `{component}` without `{key}` is discarded whole — HA logs \
                 `required key not provided` at most, and the entity simply never appears"
            );
        }
    }
    assert!(checked > 0, "no config was checked, so this proved nothing");
}

#[tokio::test]
async fn the_entity_id_seed_is_the_key_that_home_assistant_still_reads() {
    // HA deprecated `object_id` (removed in 2026.4) in favour of `default_entity_id`, which carries
    // the whole entity id including the platform prefix. It is published so that HA does not derive
    // the entity id from the name: names may change, entity ids are used in automations.
    for (fixture, topic, payload) in fleet_configs().await {
        let platform = component_of_topic(&topic);
        assert_eq!(
            payload.get("object_id"),
            None,
            "{fixture}: {topic}: `object_id` is deprecated and stops working in HA 2026.4"
        );
        let seed = payload
            .get("default_entity_id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("{fixture}: {topic}: without an entity id, HA derives one from the name")
            });
        let object_id = seed.strip_prefix(&format!("{platform}.")).unwrap_or_else(|| {
            panic!(
                "{fixture}: {topic}: `default_entity_id` is a whole entity id, so it carries the \
                 platform prefix — got `{seed}`"
            )
        });
        // Seeded from the key, not the name: the device slug, then the feature key with every
        // non-alphanumeric run collapsed. It must agree with the config topic's slot.
        let slot = topic
            .split('/')
            .nth(3)
            .expect("a discovery topic has a slot");
        assert_eq!(
            object_id,
            format!("test_device_{}", slot.to_lowercase()),
            "{fixture}: {topic}: the entity id must be the device slug plus the feature key, so a \
             corrected name cannot rename the entity"
        );
    }
}

/// Whether a config declares HA's enumeration device class.
fn is_enum_config(payload: &Value) -> bool {
    payload.get("device_class") == Some(&Value::String("enum".to_owned()))
}

/// Every discovery config for a fixture, published with the default announcement.
async fn discovery_broker(fixture: &str) -> FakeBroker {
    let profile = fixture_profile(fixture);
    let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
    let bridge = DeviceBridge::new(BASE, device_id());
    let mut broker = FakeBroker::new();
    bridge
        .publish_discovery(
            &mut broker,
            &mapping,
            &Announcement::bare(DiscoveryMode::Full),
        )
        .await
        .expect("publish discovery");
    broker
}

#[tokio::test]
async fn a_switch_states_the_payloads_it_actually_publishes() {
    // HA's MQTT switch defaults `payload_on`/`payload_off` to "ON"/"OFF", but a boolean state is
    // published as `true`/`false`. Without explicit payloads the switch reads `unknown` forever and
    // a click writes the string "ON" to a boolean feature.
    let broker = discovery_broker("hob-aes.zip").await;
    let (topic, payload) = config_named(&broker, "BSH_Common_Setting_ChildLock");
    assert!(
        topic.starts_with("homeassistant/switch/"),
        "{topic}: ChildLock is a writable boolean and should be a switch"
    );
    assert_eq!(
        payload.get("payload_on"),
        Some(&Value::String("true".to_owned())),
        "{topic}: a switch must be told the payload its own state topic carries, or it reads unknown"
    );
    assert_eq!(
        payload.get("payload_off"),
        Some(&Value::String("false".to_owned())),
        "{topic}: likewise for off"
    );
}

#[tokio::test]
async fn a_switchs_command_payload_round_trips_to_a_boolean_write() {
    // Whatever `payload_on` says is what HA publishes to the command topic, so it must be a payload
    // the write path turns into a boolean. Asserts the round trip rather than the constant.
    let profile = fixture_profile("hob-aes.zip");
    let model = profile.model();
    let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());
    let broker = discovery_broker("hob-aes.zip").await;
    let (_, payload) = config_named(&broker, "BSH_Common_Setting_ChildLock");

    for (key, expected) in [("payload_on", true), ("payload_off", false)] {
        let announced = payload
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("the switch config announces `{key}`"));
        let relative = payload
            .get("command_topic")
            .and_then(Value::as_str)
            .expect("a switch has a command topic")
            .strip_prefix(&format!("{BASE}/{}/", device_id().as_str()))
            .expect("the command topic is under the device root");
        let command = hc_core::bridge::build_write_command(model, &mapping, relative, announced)
            .unwrap_or_else(|err| {
                panic!("`{key}` = {announced:?} must be a writable payload: {err}")
            });
        assert_eq!(
            command.value,
            Value::Bool(expected),
            "`{key}` = {announced:?} must reach the appliance as a boolean, not a string — HA's \
             own default of \"ON\"/\"OFF\" would write a string to a boolean feature"
        );
    }
}

#[tokio::test]
async fn every_event_is_a_live_binary_sensor_on_all_four_appliances() {
    // Every event in the four descriptions declares no `access` attribute, so surfacing reads the
    // node kind rather than the access word. Events (`ProgramFinished`, door alarms) are surfaced
    // as binary sensors.
    for fixture in FIXTURES {
        let profile = fixture_profile(fixture);
        let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
        let events: Vec<_> = mapping
            .entries()
            .iter()
            .filter(|entry| entry.kind == hc_profile::EntityKind::Event)
            .collect();
        assert!(
            !events.is_empty(),
            "{fixture}: the fixture declares no events at all, so this test proved nothing"
        );
        for entry in events {
            assert!(
                entry.state_topic.is_some(),
                "{fixture}: {:?} must have a state topic — an event with nowhere to fire is not \
                 an event",
                entry.key
            );
            assert!(
                entry.command_topic.is_none(),
                "{fixture}: {:?} must not be writable — an event is a notification, and no event \
                 in any of the four descriptions declares write access",
                entry.key
            );
            assert_eq!(
                entry.class,
                EntityClass::BinarySensor,
                "{fixture}: {:?} is `binary_sensor (event/door)`",
                entry.key
            );
            // Events arrive disabled so they do not bury the device page; the config and state
            // topic are published either way. The program-lifecycle events in
            // `mapping::PRIMARY_LEAVES` are the exception.
            let leaf = entry
                .key
                .as_deref()
                .and_then(|key| key.rsplit('.').next())
                .unwrap_or_default();
            let primary = matches!(leaf, "ProgramFinished" | "ProgramAborted");
            assert_eq!(
                entry.enabled_by_default, primary,
                "{fixture}: {:?} — an event is enabled iff `PRIMARY_LEAVES` names it",
                entry.key
            );
        }
    }
}

/// Asserts a binary sensor's `payload_on`/`payload_off` are both members its feature can report.
fn assert_payloads_are_declared_members(
    fixture: &str,
    topic: &str,
    payload: &Value,
    entry: &EntityMapping,
) {
    // With enum members, `publish_value` emits the declared label; without them, raw JSON. So the
    // payloads are checked against a different alphabet in each case.
    let permitted: Vec<String> = if entry.options.is_empty() {
        vec!["true".to_owned(), "false".to_owned()]
    } else {
        entry.options.clone()
    };
    for key in ["payload_on", "payload_off"] {
        let announced = payload
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{fixture}: {topic}: no `{key}`"));
        assert!(
            permitted.iter().any(|option| option == announced),
            "{fixture}: {topic}: `{key}` = {announced:?} is not something this feature's state topic \
             can carry ({permitted:?}), so the entity can never match its own state and reads \
             `unknown` forever",
        );
    }
}

#[tokio::test]
async fn a_binary_sensor_announces_payloads_its_own_state_topic_can_carry() {
    // Readable two-member enumerations are not all on/off sets (`DoorState` is `Open`/`Closed` on
    // the dishwasher and `Closed`/`Open` on the fridge; others are `Left`/`Right`). The state topic
    // carries the declared label, so a binary sensor's payloads must be members the feature can
    // actually report, or it reads `unknown` permanently.
    for fixture in FIXTURES {
        let broker = discovery_broker(fixture).await;
        let profile = fixture_profile(fixture);
        let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
        for published in broker
            .discovery_publishes()
            .filter(|published| published.topic.starts_with("homeassistant/binary_sensor/"))
        {
            let payload: Value =
                serde_json::from_slice(&published.payload).expect("a discovery payload is JSON");
            let topic = &published.topic;
            let unique = payload
                .get("unique_id")
                .and_then(Value::as_str)
                .expect("every config carries a unique_id");
            let entry = mapping
                .entries()
                .iter()
                .find(|entry| entry.unique_id == unique)
                .expect("a published config maps back to its entry");
            assert_payloads_are_declared_members(fixture, topic, &payload, entry);
        }
    }
}

#[tokio::test]
async fn a_measured_value_carries_its_unit_and_device_class() {
    // A `device_class` selects HA's icon, display precision and, for temperature, unit conversion.
    // The unit comes from the content type in `refCID` (`hc_profile::ContentType`). Every row is a
    // real surfaced feature; the fixture is named because keys differ between appliances.
    let cases: [(&str, &str, &str, &str); 4] = [
        (
            "oven-tls.zip",
            "Cooking_Oven_Status_Server_Cavity_001_SetpointTemperatureCelsius",
            "temperature",
            "°C",
        ),
        (
            "hob-aes.zip",
            "BSH_Common_Option_RemainingProgramTime",
            "duration",
            "s",
        ),
        (
            "hob-aes.zip",
            "BSH_Common_Status_WiFiSignalStrength",
            "signal_strength",
            "dBm",
        ),
        ("oven-tls.zip", "Cooking_Oven_Option_Weight", "weight", "g"),
    ];
    for (fixture, slot, device_class, unit) in cases {
        let broker = discovery_broker(fixture).await;
        let (topic, payload) = config_named(&broker, slot);
        assert_eq!(
            payload.get("device_class"),
            Some(&Value::String(device_class.to_owned())),
            "{fixture}: {topic}: no device_class, so HA cannot convert or group this value"
        );
        assert_eq!(
            payload.get("unit_of_measurement"),
            Some(&Value::String(unit.to_owned())),
            "{fixture}: {topic}: no unit, so the value reads as a bare number"
        );
    }
}

#[tokio::test]
async fn a_percentage_carries_its_unit_without_claiming_a_device_class() {
    // HA has no `percentage` device class (the nearest, `battery`, means something else), so
    // `ProgramProgress` carries a unit and no class.
    let broker = discovery_broker("hob-aes.zip").await;
    let (topic, payload) = config_named(&broker, "BSH_Common_Option_ProgramProgress");
    assert_eq!(
        payload.get("unit_of_measurement"),
        Some(&Value::String("%".to_owned())),
        "{topic}: a percentage's unit is derivable"
    );
    assert_eq!(
        payload.get("device_class"),
        None,
        "{topic}: HA has no percentage device class, and `battery` is not a near-enough miss"
    );
}

#[tokio::test]
async fn a_numeric_sensor_is_given_a_state_class_so_it_gets_statistics() {
    // Without `state_class` HA records no long-term statistics. `measurement` for a value that moves
    // both ways, `total_increasing` for a counter such as
    // `BSH.Common.Status.Program.All.Count.Started`.
    let broker = discovery_broker("oven-tls.zip").await;
    let (topic, payload) = config_named(
        &broker,
        "Cooking_Oven_Status_Server_Cavity_001_SetpointTemperatureCelsius",
    );
    assert_eq!(
        payload.get("state_class"),
        Some(&Value::String("measurement".to_owned())),
        "{topic}: a temperature is a measurement"
    );

    let broker = discovery_broker("dishwasher-tls.zip").await;
    let (topic, payload) = config_named(&broker, "BSH_Common_Status_Program_All_Count_Started");
    assert_eq!(
        payload.get("state_class"),
        Some(&Value::String("total_increasing".to_owned())),
        "{topic}: a lifetime count only rises; `measurement` would have HA average a counter"
    );
}

#[tokio::test]
async fn an_enumeration_never_claims_a_numeric_unit() {
    // An enumeration's state topic carries a label, not a number, so it must have neither a unit
    // nor a `state_class`. The derivation keys on the content type, not on "is it a sensor".
    for (fixture, topic, payload) in fleet_configs().await {
        if !is_enum_config(&payload) {
            continue;
        }
        assert_eq!(
            payload.get("unit_of_measurement"),
            None,
            "{fixture}: {topic}: an enum sensor publishes a label, not a measurement"
        );
        assert_eq!(
            payload.get("state_class"),
            None,
            "{fixture}: {topic}: HA cannot compute statistics over labels"
        );
    }
}

/// Every key HA accepts only on some platforms, and the platforms it accepts each on. HA discards
/// the whole entity on a key its platform does not take.
const PLATFORM_ONLY_KEYS: [(&str, &[&str]); 3] = [
    ("unit_of_measurement", &["sensor", "number"]),
    ("state_class", &["sensor"]),
    ("options", &["sensor", "select"]),
];

#[tokio::test]
async fn no_config_carries_a_key_its_own_platform_rejects() {
    // HA refuses a config carrying a key its platform does not know, so no platform-specific key
    // may land on the wrong platform, whatever the classifier or the operator catalogue produces.
    for (fixture, topic, payload) in fleet_configs().await {
        let platform = component_of_topic(&topic);
        let misplaced = PLATFORM_ONLY_KEYS
            .iter()
            .find(|(key, allowed)| payload.get(*key).is_some() && !allowed.contains(&platform));
        assert!(
            misplaced.is_none(),
            "{fixture}: {topic}: `{}` is not a `{platform}` key, and HA discards the whole entity \
             rather than ignoring it",
            misplaced.map_or("", |(key, _)| key)
        );
    }
}

/// The `device_class` tokens each platform accepts, restricted to the ones this crate can emit.
///
/// `mapping::DeviceClass` emits seven tokens (`enum`, `temperature`, `duration`,
/// `signal_strength`, `weight`, `power`, `energy`): all are sensor classes, all but `enum` are
/// number classes, and none is accepted by `button`, `switch`, `binary_sensor` or `select`. A
/// binary sensor takes only its own vocabulary, listed here in full. All six
/// components are listed, so a missing component fails the lookup rather than being skipped.
const DEVICE_CLASSES_BY_PLATFORM: [(&str, &[&str]); 6] = [
    (
        "sensor",
        &[
            "enum",
            "temperature",
            "duration",
            "signal_strength",
            "weight",
            "power",
            "energy",
        ],
    ),
    (
        "number",
        &[
            "temperature",
            "duration",
            "signal_strength",
            "weight",
            "power",
            "energy",
        ],
    ),
    ("switch", &[]),
    ("select", &[]),
    (
        "binary_sensor",
        &[
            "battery",
            "battery_charging",
            "carbon_monoxide",
            "cold",
            "connectivity",
            "door",
            "garage_door",
            "gas",
            "heat",
            "light",
            "lock",
            "moisture",
            "motion",
            "moving",
            "occupancy",
            "opening",
            "plug",
            "power",
            "presence",
            "problem",
            "running",
            "safety",
            "smoke",
            "sound",
            "tamper",
            "update",
            "vibration",
            "window",
        ],
    ),
    ("button", &[]),
];

#[tokio::test]
async fn no_config_claims_a_device_class_its_own_platform_has_no_word_for() {
    // A `<command>` button whose `refCID` decodes to `timeSpan` must not carry `device_class:
    // duration`: HA rejects a button with any class outside `identify`/`restart`/`update` and
    // discards the entity.
    for (fixture, topic, payload) in fleet_configs().await {
        let Some(class) = payload.get("device_class") else {
            continue;
        };
        let platform = component_of_topic(&topic);
        let (_, allowed) = DEVICE_CLASSES_BY_PLATFORM
            .iter()
            .find(|(name, _)| *name == platform)
            .unwrap_or_else(|| {
                panic!("{fixture}: {topic}: no device-class table for `{platform}`")
            });
        let class = class.as_str().expect("a device_class is a string");
        assert!(
            allowed.contains(&class),
            "{fixture}: {topic}: `{class}` is not in a `{platform}`'s device-class vocabulary, and \
             HA discards the whole entity rather than ignoring the key"
        );
    }
}

#[tokio::test]
async fn device_class_enum_appears_only_on_a_sensor_and_only_beside_options() {
    // `enum` is a sensor-only device class; HA discards a binary sensor that carries it. On a
    // sensor, `device_class: enum` and `options` imply each other.
    for (fixture, topic, payload) in fleet_configs().await {
        if !is_enum_config(&payload) {
            continue;
        }
        assert_eq!(
            component_of_topic(&topic),
            "sensor",
            "{fixture}: {topic}: `enum` is a sensor-only device class; HA discards the entity"
        );
        assert!(
            payload.get("options").is_some(),
            "{fixture}: {topic}: an `enum` sensor with no `options` is not a valid config"
        );
    }
}

#[tokio::test]
async fn a_sensor_that_offers_options_declares_the_enum_class_they_require() {
    // HA also requires `options` to come with `device_class: enum`. `BSH.Common.Root.ActiveProgram`
    // is a programme pointer whose `options` come from the `<program>` list while its value type is
    // not an enumeration, so the class must still be set.
    let mut checked = 0_usize;
    for (fixture, topic, payload) in fleet_configs().await {
        if component_of_topic(&topic) != "sensor" || payload.get("options").is_none() {
            continue;
        }
        checked += 1;
        assert!(
            is_enum_config(&payload),
            "{fixture}: {topic}: a sensor with `options` and no `device_class: enum` is refused \
             whole — the vocabulary and the class imply each other"
        );
    }
    assert!(
        checked > 0,
        "no sensor offered options, so this proved nothing"
    );
}

#[tokio::test]
async fn the_operationally_important_features_stay_prominent_and_enabled() {
    // HA lays a device page out by `entity_category` and `enabled_by_default`: no category for
    // Controls/Sensors, `config` for Configuration, `diagnostic` for Diagnostic, and disabled
    // entities collapsed. These operationally important features must be uncategorised and
    // enabled.
    let cases: [(&str, &str); 4] = [
        ("hob-aes.zip", "BSH_Common_Status_OperationState"),
        ("hob-aes.zip", "BSH_Common_Option_RemainingProgramTime"),
        ("hob-aes.zip", "BSH_Common_Option_ProgramProgress"),
        ("dishwasher-tls.zip", "BSH_Common_Status_DoorState"),
    ];
    for (fixture, slot) in cases {
        let broker = discovery_broker(fixture).await;
        let (topic, payload) = config_named(&broker, slot);
        assert_eq!(
            payload.get("entity_category"),
            None,
            "{fixture}: {topic}: a primary feature must stay in HA's prominent section, not be \
             filed under Configuration or Diagnostic"
        );
        assert_ne!(
            payload.get("enabled_by_default"),
            Some(&Value::Bool(false)),
            "{fixture}: {topic}: …and must not arrive switched off"
        );
    }
}

#[tokio::test]
async fn the_long_tail_is_categorised_and_arrives_disabled() {
    // Everything outside the primary set: a control HA can operate goes to Configuration, a reading
    // to Diagnostic, and both arrive disabled. Nothing is dropped and no topic changes.
    let cases: [(&str, &str, &str); 3] = [
        // A writable boolean setting: a control an operator may want, but not a headline.
        (
            "hob-aes.zip",
            "BSH_Common_Setting_AllowConsumerInsights",
            "config",
        ),
        // A read-only status: information, never a control.
        (
            "hob-aes.zip",
            "BSH_Common_Status_SoftwareUpdateTransactionID",
            "diagnostic",
        ),
        // The ~30 favourites per oven. Writable strings, but published as sensors, so Diagnostic:
        // the category follows the published `class`, not `access`, because HA's Configuration
        // section is for entities that change the device.
        (
            "oven-tls.zip",
            "BSH_Common_Setting_Favorite_001_Name",
            "diagnostic",
        ),
    ];
    for (fixture, slot, category) in cases {
        let broker = discovery_broker(fixture).await;
        let (topic, payload) = config_named(&broker, slot);
        assert_eq!(
            payload.get("entity_category"),
            Some(&Value::String(category.to_owned())),
            "{fixture}: {topic}: belongs in HA's `{category}` section"
        );
        assert_eq!(
            payload.get("enabled_by_default"),
            Some(&Value::Bool(false)),
            "{fixture}: {topic}: a long-tail feature arrives disabled — published, discoverable, \
             one click from on, and not in the way"
        );
    }
}

#[tokio::test]
async fn disabling_the_long_tail_changes_no_topic_and_drops_no_config() {
    // Categorising and disabling is presentation: every surfaceable feature still gets a discovery
    // config and every value-bearing feature still gets a state topic. "Disabled" must not be
    // implemented as "not published".
    for fixture in FIXTURES {
        let profile = fixture_profile(fixture);
        let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
        let broker = discovery_broker(fixture).await;
        let published = broker.discovery_publishes().count();
        let surfaceable = mapping
            .entries()
            .iter()
            .filter(|entry| {
                !entry.kind.is_container()
                    && (entry.state_topic.is_some() || entry.command_topic.is_some())
            })
            .count();
        assert_eq!(
            published,
            surfaceable + hc_core::discovery::synthetic_control_topics(&mapping).len(),
            "{fixture}: every surfaceable feature must still have a config — a disabled entity is \
             published, not withheld"
        );
        let disabled = broker
            .discovery_publishes()
            .filter(|p| {
                serde_json::from_slice::<Value>(&p.payload)
                    .expect("a discovery payload is JSON")
                    .get("enabled_by_default")
                    == Some(&Value::Bool(false))
            })
            .count();
        assert!(
            disabled > 0,
            "{fixture}: nothing was disabled, so this test proved nothing"
        );
        assert!(
            disabled < published,
            "{fixture}: everything was disabled, which is not a tidier UI but an empty one"
        );
    }
}

/// The HA component token of a discovery topic: `homeassistant/<component>/…/config`.
fn component_of_topic(topic: &str) -> &str {
    topic
        .split('/')
        .nth(1)
        .unwrap_or_else(|| panic!("a discovery topic has a component segment: {topic}"))
}

#[tokio::test]
async fn every_sensor_config_clamps_to_home_assistants_state_length_cap() {
    // HA refuses a state longer than 255 characters outright (the recorder's column width) and the
    // entity goes `unknown`. Only `sensor` can reach that limit. The clamp is a value template in
    // the discovery config, so the raw state topic keeps the full value for other consumers, and
    // the template runs before HA's `None` comparison, so `None` survives it.
    for name in FIXTURES {
        let profile = fixture_profile(name);
        let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
        let bridge = DeviceBridge::new(BASE, device_id());
        let mut broker = FakeBroker::new();

        bridge
            .publish_discovery(
                &mut broker,
                &mapping,
                &Announcement::bare(DiscoveryMode::Full),
            )
            .await
            .expect("publish discovery");

        let sensor_configs = broker
            .discovery_publishes()
            .filter(|published| component_of_topic(&published.topic) == "sensor");

        let mut checked = 0_usize;
        for published in sensor_configs {
            let payload: Value =
                serde_json::from_slice(&published.payload).expect("a discovery payload is JSON");
            assert_eq!(
                payload.get("value_template"),
                Some(&Value::String("{{ value[:255] }}".to_owned())),
                "{name}: {} must clamp to HA's 255-character state cap, or a longer value is \
                 rejected whole and the entity reads unknown",
                published.topic
            );
            checked += 1;
        }
        assert!(checked > 0, "{name}: no sensor config was published");
    }
}

/// A fake write transport that records every [`WriteCommand`] that reaches it. The `sent` log is a
/// shared handle so a test can inspect it after the transport has moved into a [`GuardedWriter`].
#[derive(Clone, Default)]
struct RecordingTransport {
    sent: Arc<Mutex<Vec<WriteCommand>>>,
}

impl RecordingTransport {
    fn new() -> Self {
        Self::default()
    }

    fn count(&self) -> usize {
        self.sent.lock().expect("lock").len()
    }

    fn last(&self) -> Option<WriteCommand> {
        self.sent.lock().expect("lock").last().cloned()
    }
}

#[async_trait]
impl WriteTransport for RecordingTransport {
    async fn transmit(&mut self, command: &WriteCommand) -> Result<(), String> {
        self.sent.lock().expect("lock").push(command.clone());
        Ok(())
    }
}

/// The mapping entry for a feature key, or a panic naming the missing key.
fn entry_for<'m>(mapping: &'m DeviceMapping, key: &str) -> &'m EntityMapping {
    mapping
        .entries()
        .iter()
        .find(|entry| entry.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("no mapping entry for {key}"))
}

/// The first writable numeric entry (a `number`) in a mapping, whose `…/set` payload parses without
/// an enum lookup.
fn first_writable_number(mapping: &DeviceMapping) -> &EntityMapping {
    mapping
        .entries()
        .iter()
        .find(|entry| entry.command_topic.is_some() && entry.class == EntityClass::Number)
        .expect("a writable numeric feature")
}

#[tokio::test]
async fn set_write_routes_through_guard() {
    // An incoming `…/set` reaches a device write only via the GuardedWriter. A dishwasher select
    // write reaches the transport with its HA label converted to the wire integer; so does the
    // same set for the hob.
    let dishwasher = fixture_profile("dishwasher-tls.zip");
    let dw_model = dishwasher.model();
    let dw_map = DeviceMapping::build(&device_id(), dw_model, &Excludes::none());

    // A writable enum on the dishwasher: PowerState (Off/On). Compute the wire integer for "On"
    // from the model, independently of the conversion under test.
    let power = entry_for(&dw_map, "BSH.Common.Setting.PowerState");
    assert_eq!(
        power.class,
        EntityClass::Select,
        "PowerState is a writable enum"
    );
    let power_set = power.command_topic.clone().expect("PowerState is writable");
    let power_entity = dw_model.entity(power.uid).expect("PowerState entity");
    let enum_type = dw_model
        .enum_type_of(power_entity)
        .expect("PowerState enum");
    let on_wire = enum_type
        .members
        .iter()
        .find(|(_, name)| name.as_str() == "On")
        .map(|(value, _)| *value)
        .expect("On is a declared member");

    let dw_recorder = RecordingTransport::new();
    let mut dw_writer = GuardedWriter::new(dw_recorder.clone());

    handle_set(
        &mut dw_writer,
        &hc_core::bridge::SetContext::new(dw_model, &dw_map),
        &power_set,
        "On",
    )
    .await
    .expect("dishwasher set routes through the guard and transmits");

    assert_eq!(
        dw_recorder.count(),
        1,
        "the dishwasher set must reach the transport exactly once"
    );
    assert_eq!(
        dw_recorder.last().expect("a transmitted command").value,
        Value::from(on_wire),
        "the HA label 'On' must be converted to its wire integer via the enum type"
    );

    // The same set path reaches the transport for the hob too.
    let hob = fixture_profile("hob-aes.zip");
    let hob_model = hob.model();
    let hob_map = DeviceMapping::build(&device_id(), hob_model, &Excludes::none());
    let hob_number = first_writable_number(&hob_map);
    let hob_set = hob_number
        .command_topic
        .clone()
        .expect("a writable hob feature");

    let hob_recorder = RecordingTransport::new();
    let mut hob_writer = GuardedWriter::new(hob_recorder.clone());

    handle_set(
        &mut hob_writer,
        &hc_core::bridge::SetContext::new(hob_model, &hob_map),
        &hob_set,
        "1",
    )
    .await
    .expect("no gate remains that could refuse a cooktop write");
    assert_eq!(
        hob_recorder.count(),
        1,
        "the hob set reaches the transport exactly once, exactly like the dishwasher's"
    );
}

/// `build_write_command` produces a transmittable cooktop command from a plain MQTT payload, and the
/// guarded and unguarded paths agree on it.
#[tokio::test]
async fn a_hob_set_command_transmits_with_or_without_the_guard() {
    let hob = fixture_profile("hob-aes.zip");
    let hob_model = hob.model();
    let hob_map = DeviceMapping::build(&device_id(), hob_model, &Excludes::none());
    let hob_number = first_writable_number(&hob_map);
    let hob_set = hob_number
        .command_topic
        .clone()
        .expect("a writable hob feature");

    let command = hc_core::build_write_command(hob_model, &hob_map, &hob_set, "1")
        .expect("the hob command builds fine — the danger is real, not a parse failure");

    // The un-guarded path: transmit straight to the transport.
    let mut recorder = RecordingTransport::new();
    recorder
        .transmit(&command)
        .await
        .expect("an un-guarded transmit succeeds");

    assert_eq!(
        recorder.count(),
        1,
        "the un-guarded transmit reaches the transport — and so does the guarded one, which \
         `set_write_routes_through_guard` asserts directly"
    );
}

/// The payloads (as UTF-8 strings) published to `topic`, in order.
fn payloads_on(broker: &FakeBroker, topic: &str) -> Vec<String> {
    broker
        .publishes
        .iter()
        .filter(|p| p.topic == topic)
        .map(|p| String::from_utf8(p.payload.clone()).expect("utf8 payload"))
        .collect()
}

#[tokio::test]
async fn availability_and_connection_state() {
    use hc_core::ConnectionState;
    use hc_core::bridge::{AVAILABILITY_OFFLINE, AVAILABILITY_ONLINE};

    let bridge = DeviceBridge::new(BASE, device_id());
    let avail = bridge.availability_topic();
    let conn = bridge.connection_state_topic();

    // LWT: the broker's will turns the device `offline` on an unclean disconnect.
    let will = bridge.last_will();
    assert_eq!(
        will.topic, avail,
        "the will publishes to the availability topic"
    );
    assert_eq!(
        will.payload, AVAILABILITY_OFFLINE,
        "the LWT payload models the device going offline"
    );
    assert!(
        will.retain,
        "the will must be retained so a late subscriber learns it"
    );

    let mut broker = FakeBroker::with_will(will.clone());
    assert_eq!(
        broker.last_will.as_ref(),
        Some(&will),
        "the broker holds the connect-time will (LWT)"
    );

    // Coming online publishes `online` (retained) on the availability topic.
    bridge
        .publish_availability(&mut broker, true)
        .await
        .expect("publish online");
    assert_eq!(
        payloads_on(&broker, &avail),
        vec![String::from_utf8(AVAILABILITY_ONLINE.to_vec()).unwrap()],
        "coming online publishes `online`"
    );

    // connection_state transitions map straight from ConnectionState::as_str.
    let states = [
        ConnectionState::Connecting,
        ConnectionState::Handshake,
        ConnectionState::Connected,
        ConnectionState::Reconnecting,
        ConnectionState::Offline,
    ];
    for state in states {
        bridge
            .publish_connection_state(&mut broker, state)
            .await
            .expect("publish connection_state");
    }
    assert_eq!(
        payloads_on(&broker, &conn),
        states
            .iter()
            .map(|s| s.as_str().to_owned())
            .collect::<Vec<_>>(),
        "every ConnectionState transition is published as its token"
    );

    // A drop then reconnect republishes: offline, then online again.
    bridge
        .publish_availability(&mut broker, false)
        .await
        .expect("publish offline");
    bridge
        .publish_availability(&mut broker, true)
        .await
        .expect("publish online again");
    let availability = payloads_on(&broker, &avail);
    assert_eq!(
        availability.last().map(String::as_str),
        Some("online"),
        "reconnect republishes `online`"
    );
    assert!(
        availability.contains(&"offline".to_owned()),
        "the drop published `offline`"
    );
}

/// Every string a broker emitted: publish topics and payloads, subscription filters, and the will.
fn all_emitted_strings(broker: &FakeBroker) -> Vec<String> {
    let mut out = Vec::new();
    for p in &broker.publishes {
        out.push(p.topic.clone());
        if let Ok(text) = String::from_utf8(p.payload.clone()) {
            out.push(text);
        }
    }
    out.extend(broker.subscriptions.iter().cloned());
    if let Some(will) = &broker.last_will {
        out.push(will.topic.clone());
        if let Ok(text) = String::from_utf8(will.payload.clone()) {
            out.push(text);
        }
    }
    out
}

/// Drives a full device announce (discovery + state + set-subscribe + availability + every
/// `connection_state`) for a given device slug, and returns the resulting fake broker.
async fn announce_everything(slug: &str, model: &EntityModel) -> FakeBroker {
    let device = DeviceId::new(slug);
    let mapping = DeviceMapping::build(&device, model, &Excludes::none());
    let bridge = DeviceBridge::new(BASE, device);
    let mut broker = FakeBroker::with_will(bridge.last_will());
    bridge
        .announce(
            &mut broker,
            &mapping,
            &Announcement::bare(DiscoveryMode::Full),
        )
        .await
        .expect("announce");
    bridge
        .publish_availability(&mut broker, true)
        .await
        .expect("availability");
    for state in [
        hc_core::ConnectionState::Connecting,
        hc_core::ConnectionState::Connected,
        hc_core::ConnectionState::Offline,
    ] {
        bridge
            .publish_connection_state(&mut broker, state)
            .await
            .expect("connection_state");
    }
    broker
}

#[tokio::test]
async fn no_identifier_in_topics_or_discovery() {
    // No emitted topic or discovery payload may contain an appliance identifier. First prove a
    // sentinel is detectable when the slug is the sentinel, then prove it is absent everywhere with
    // an operator slug.
    // A stand-in for a real appliance identifier (haId/serial/mac form).
    const SENTINEL: &str = "001122334455-HCDEVID-SENTINEL";

    let hob = fixture_profile("hob-aes.zip");
    let model = hob.model();

    // Detectability: if the device slug were the identifier, it would appear all over the surface.
    let leaky = announce_everything(SENTINEL, model).await;
    let leaked = all_emitted_strings(&leaky);
    assert!(
        leaked.iter().any(|s| s.contains(SENTINEL)),
        "the sweep must be able to catch a leak — a slug == identifier must surface it \
         (otherwise the absence test below is vacuous)"
    );
    // Specifically, it reaches the unique_id inside discovery payloads.
    assert!(
        leaky
            .discovery_publishes()
            .filter_map(|p| String::from_utf8(p.payload).ok())
            .any(|payload| payload.contains(SENTINEL)),
        "a leaked identifier would reach the discovery unique_id"
    );

    // An operator slug never carries the identifier anywhere.
    let safe = announce_everything("kitchen-hob", model).await;
    let emitted = all_emitted_strings(&safe);
    assert!(
        !emitted.is_empty(),
        "the swept surface must be non-empty (else the absence check is vacuous)"
    );
    for s in &emitted {
        assert!(
            !s.contains(SENTINEL),
            "no emitted topic/payload may contain the appliance identifier — found in {s:?}"
        );
    }
    // And the unique_ids are the safe-slug form, never the identifier.
    let uid_leaks = safe
        .discovery_publishes()
        .filter_map(|p| String::from_utf8(p.payload).ok())
        .filter(|payload| payload.contains(SENTINEL))
        .count();
    assert_eq!(
        uid_leaks, 0,
        "no discovery unique_id may carry the identifier"
    );
}

// =================================================================================================
// Publishing appliance values.
// =================================================================================================

/// The uid and enum member the dishwasher fixture declares for `BSH.Common.Setting.Time.DisplayMode`,
/// resolved from the fixture rather than hard-coded.
fn enum_feature(model: &EntityModel) -> (u16, i64, String) {
    let entity = model
        .entities()
        .values()
        .find(|entity| {
            entity.key.as_deref() == Some("BSH.Common.Setting.Time.DisplayMode")
                && model.enum_type_of(entity).is_some()
        })
        .expect("the dishwasher declares Time.DisplayMode as an enumeration");
    let enum_type = model.enum_type_of(entity).expect("checked above");
    let value = *enum_type
        .declared_values
        .first()
        .expect("the appliance declares at least one member");
    let label = enum_type
        .member(value)
        .expect("the mapping names the declared member")
        .to_owned();
    (entity.uid, value, label)
}

#[tokio::test]
async fn a_value_is_published_to_its_feature_state_topic() {
    let profile = fixture_profile("dishwasher-tls.zip");
    let model = profile.model();
    let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());
    let bridge = DeviceBridge::new(BASE, device_id());

    // A numeric feature, so the value passes through unchanged and only the routing is under test.
    let numeric = mapping
        .entries()
        .iter()
        .find(|entry| {
            entry.state_topic.is_some()
                && model
                    .entity(entry.uid)
                    .is_some_and(|entity| model.enum_type_of(entity).is_none())
        })
        .expect("the dishwasher has a readable non-enumeration feature");
    let topic = format!(
        "{BASE}/{}/{}",
        device_id().as_str(),
        numeric.state_topic.as_deref().expect("checked above")
    );

    let mut broker = FakeBroker::new();
    bridge
        .publish_value(
            &mut broker,
            &mapping,
            model,
            &hc_proto::ValueUpdate {
                uid: u64::from(numeric.uid),
                value: Value::from(42),
                description: hc_proto::DescriptionUpdate::default(),
            },
        )
        .await
        .expect("publishing a value");

    let published: Vec<_> = broker
        .publishes
        .iter()
        .filter(|p| p.topic == topic)
        .collect();
    assert_eq!(
        published.len(),
        1,
        "the value must reach exactly this feature's state topic; recorded topics: {:?}",
        broker
            .publishes
            .iter()
            .map(|p| &p.topic)
            .collect::<Vec<_>>()
    );
    let only = published.first().expect("one was just asserted");
    assert_eq!(
        only.payload,
        b"42".to_vec(),
        "the payload is the value itself, not a JSON envelope around it"
    );
    assert!(
        only.retain,
        "retained: a late HA subscriber must learn the current value without waiting for a change"
    );
}

#[tokio::test]
async fn an_enum_value_is_published_as_its_declared_label() {
    // HA's `select` renders `options`, which are member names, so the state topic carries the
    // label rather than the wire ordinal.
    let profile = fixture_profile("dishwasher-tls.zip");
    let model = profile.model();
    let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());
    let bridge = DeviceBridge::new(BASE, device_id());
    let (uid, ordinal, label) = enum_feature(model);

    let mut broker = FakeBroker::new();
    bridge
        .publish_value(
            &mut broker,
            &mapping,
            model,
            &hc_proto::ValueUpdate {
                uid: u64::from(uid),
                value: Value::from(ordinal),
                description: hc_proto::DescriptionUpdate::default(),
            },
        )
        .await
        .expect("publishing an enum value");

    let payloads: Vec<String> = broker
        .state_publishes()
        .map(|p| String::from_utf8_lossy(&p.payload).into_owned())
        .collect();
    assert_eq!(
        payloads,
        vec![label.clone()],
        "the declared member NAME must be published, not the wire ordinal {ordinal}"
    );
    assert_ne!(
        payloads.first().map(String::as_str),
        Some(ordinal.to_string().as_str()),
        "an ordinal on a state topic can never match the `options` HA was given"
    );
}

#[tokio::test]
async fn a_null_value_is_published_as_home_assistants_none_sentinel() {
    // A JSON `null` is published as `None`, which HA's sensor checks before the options list and
    // treats as "no value". The literal `null` would be rejected as an invalid option and the entity
    // would keep its stale value.
    let profile = fixture_profile("dishwasher-tls.zip");
    let model = profile.model();
    let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());
    let bridge = DeviceBridge::new(BASE, device_id());
    // An enumeration feature, because HA validates that class against `options`.
    let (uid, _, _) = enum_feature(model);

    let mut broker = FakeBroker::new();
    bridge
        .publish_value(
            &mut broker,
            &mapping,
            model,
            &hc_proto::ValueUpdate {
                uid: u64::from(uid),
                value: Value::Null,
                description: hc_proto::DescriptionUpdate::default(),
            },
        )
        .await
        .expect("publishing a null value");

    let payloads: Vec<String> = broker
        .state_publishes()
        .map(|p| String::from_utf8_lossy(&p.payload).into_owned())
        .collect();
    assert_eq!(
        payloads,
        vec!["None".to_owned()],
        "a reported null must reach HA as its `None` sentinel; `null` is refused against `options` \
         and leaves the entity showing a stale value"
    );

    // `None` is an HA convention and stops at the MQTT edge: `rendered_value` is shared with the
    // REST and SSE surfaces, where it keeps rendering the JSON it was handed.
    assert_eq!(
        hc_core::bridge::rendered_value(model, uid, &Value::Null),
        "null",
        "the shared renderer must not learn HA's vocabulary"
    );
}

#[tokio::test]
async fn a_state_outside_the_announced_options_publishes_none_not_the_raw_value() {
    // An undeclared ordinal on a `select` is rejected by HA against `options`. The hob's
    // `ZoneSelector` members are 100/200/300/400 and it reports 0 when no zone is selected; that is
    // published as `None`.
    let profile = fixture_profile("hob-aes.zip");
    let model = profile.model();
    let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());
    let bridge = DeviceBridge::new(BASE, device_id());
    let entry = entry_for(&mapping, "Cooking.Hob.Option.ZoneSelector");
    // Non-vacuous: the value under test really is outside the list the announce published.
    assert!(
        !entry.options.iter().any(|member| member == "0"),
        "the hob must declare no member `0`, or this test proves nothing"
    );

    let mut broker = FakeBroker::new();
    bridge
        .publish_value(
            &mut broker,
            &mapping,
            model,
            &hc_proto::ValueUpdate {
                uid: u64::from(entry.uid),
                value: Value::from(0),
                description: hc_proto::DescriptionUpdate::default(),
            },
        )
        .await
        .expect("publishing an undeclared ordinal");

    let payloads: Vec<String> = broker
        .state_publishes()
        .map(|p| String::from_utf8_lossy(&p.payload).into_owned())
        .collect();
    assert_eq!(
        payloads,
        vec!["None".to_owned()],
        "an ordinal the appliance never named is not one of the `options` HA was given, so the raw \
         value is discarded on arrival and the entity keeps a stale zone"
    );

    // Off MQTT the ordinal is unchanged, so an HTTP caller can resolve it itself.
    assert_eq!(
        hc_core::bridge::rendered_value(model, entry.uid, &Value::from(0)),
        "0",
        "the shared renderer must not learn HA's vocabulary"
    );
}

#[tokio::test]
async fn a_value_for_an_unmapped_uid_is_not_published() {
    // An update for a uid the mapping has no state topic for is dropped rather than published to a
    // guessed topic.
    let profile = fixture_profile("dishwasher-tls.zip");
    let model = profile.model();
    let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());
    let bridge = DeviceBridge::new(BASE, device_id());

    let unmapped = mapping
        .entries()
        .iter()
        .map(|entry| u64::from(entry.uid))
        .max()
        .map_or(0xFFFF, |max| max + 1);

    let mut broker = FakeBroker::new();
    bridge
        .publish_value(
            &mut broker,
            &mapping,
            model,
            &hc_proto::ValueUpdate {
                uid: unmapped,
                value: Value::from(7),
                description: hc_proto::DescriptionUpdate::default(),
            },
        )
        .await
        .expect("an unmapped uid is not an error");

    assert!(
        broker.publishes.is_empty(),
        "no topic may gain a publish for a uid the mapping does not map: {:?}",
        broker
            .publishes
            .iter()
            .map(|p| &p.topic)
            .collect::<Vec<_>>()
    );
}

/// No surfaced feature key names an appliance identifier, so no feature value can publish one.
///
/// The topic and payload-metadata sweeps cannot see an identifier carried as a feature's value. A
/// key such as `…SerialNumber` is the proxy. Proven for the four scrubbed fixtures; a future
/// firmware exposing such a feature fails here.
#[test]
fn no_surfaced_feature_key_names_an_l10_identifier() {
    // Identifier names (psk, iv, serial, mac, shipSki, deviceID), matched as case-insensitive
    // substrings against every surfaced feature key.
    const FORBIDDEN: [&str; 8] = [
        "serialnumber",
        "macaddress",
        "shipski",
        "deviceid",
        "haid",
        "psk",
        "privatekey",
        "secretkey",
    ];

    let mut surfaced = 0_usize;
    let mut offenders: Vec<String> = Vec::new();

    for name in FIXTURES {
        let profile = fixture_profile(name);
        let model = profile.model();
        let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());
        for entry in mapping.entries() {
            let Some(key) = entry.key.as_deref() else {
                continue;
            };
            // Only a feature with a topic can carry a value anywhere.
            if entry.state_topic.is_none() && entry.command_topic.is_none() {
                continue;
            }
            surfaced += 1;
            let lowered = key.to_lowercase();
            if FORBIDDEN
                .iter()
                .any(|forbidden| lowered.contains(forbidden))
            {
                offenders.push(format!("{name}: {key}"));
            }
        }
    }

    // Non-vacuity: the scan must have looked at a real, large surface.
    assert!(
        surfaced > 400,
        "the audit must cover a substantial feature surface across the four descriptions, saw \
         {surfaced} — a small number means the walk is wrong, not that the appliances are simple. \
         Measured at 540 across the four scrubbed descriptions when this was written"
    );
    // And the matcher itself works: a synthetic key must be caught.
    let sentinel = "BSH.Common.Root.SerialNumber".to_lowercase();
    assert!(
        FORBIDDEN.iter().any(|f| sentinel.contains(f)),
        "the matcher must catch the obvious case, or the absence result below is vacuous"
    );

    assert!(
        offenders.is_empty(),
        "a feature whose VALUE would be an identifier is surfaced on a topic, so publishing \
         values would leak it in the payload of a legitimate topic — every existing sweep would \
         still pass. Offenders: {offenders:?}"
    );
}

/// The `BSH.Common.Root.ActiveProgram` / `…SelectedProgram` entry, which points at a program by uid.
fn program_pointer<'m>(mapping: &'m DeviceMapping, leaf: &str) -> &'m EntityMapping {
    mapping
        .entries()
        .iter()
        .find(|entry| entry.key.as_deref() == Some(leaf))
        .unwrap_or_else(|| panic!("the fixture declares no `{leaf}`"))
}

#[tokio::test]
async fn a_program_pointer_publishes_the_programs_name_not_its_uid() {
    // `BSH.Common.Root.ActiveProgram` and `…SelectedProgram` hold the uid of a program, which means
    // nothing to a person; the appliance's own `<program>` list names it.
    let profile = fixture_profile("oven-tls.zip");
    let model = profile.model();
    let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());
    let bridge = DeviceBridge::new(BASE, device_id());
    let pointer = program_pointer(&mapping, "BSH.Common.Root.SelectedProgram");
    let program = mapping
        .programs()
        .iter()
        .find(|program| program.key.is_some())
        .expect("the oven declares named programmes");

    let mut broker = FakeBroker::new();
    bridge
        .publish_value(
            &mut broker,
            &mapping,
            model,
            &hc_proto::ValueUpdate {
                uid: u64::from(pointer.uid),
                value: Value::from(program.uid),
                description: hc_proto::DescriptionUpdate::default(),
            },
        )
        .await
        .expect("publish value");

    let topic = format!(
        "{BASE}/{}/{}",
        device_id().as_str(),
        pointer.state_topic.as_deref().expect("it is readable")
    );
    assert_eq!(
        payloads_on(&broker, &topic),
        vec![program.key.clone().expect("checked above")],
        "a program pointer must publish the programme's own key, not the uid behind it"
    );
}

#[tokio::test]
async fn an_unresolved_program_pointer_publishes_none_rather_than_a_bare_number() {
    // Uid 0, or a uid the description does not declare, is no programme and publishes `None`, which
    // HA shows as `unknown`. MQTT only: `rendered_value` itself passes the uid through.
    let profile = fixture_profile("oven-tls.zip");
    let model = profile.model();
    let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());
    let bridge = DeviceBridge::new(BASE, device_id());
    let pointer = program_pointer(&mapping, "BSH.Common.Root.ActiveProgram");
    let topic = format!(
        "{BASE}/{}/{}",
        device_id().as_str(),
        pointer.state_topic.as_deref().expect("it is readable")
    );

    for unresolved in [0_i64, 0xFFFF] {
        let mut broker = FakeBroker::new();
        bridge
            .publish_value(
                &mut broker,
                &mapping,
                model,
                &hc_proto::ValueUpdate {
                    uid: u64::from(pointer.uid),
                    value: Value::from(unresolved),
                    description: hc_proto::DescriptionUpdate::default(),
                },
            )
            .await
            .expect("publish value");
        assert_eq!(
            payloads_on(&broker, &topic),
            vec!["None".to_owned()],
            "uid {unresolved} names no programme, so the entity must read unknown rather than \
             showing a number as if it were a programme"
        );
    }
}

#[tokio::test]
async fn a_readable_boolean_is_a_binary_sensor_not_an_anonymous_sensor() {
    // A read-only boolean is a `binary_sensor` reading `on`/`off`, not a text sensor reading
    // `true`. The fridge declares `access="none"` on every setting, so none of its booleans are
    // writable switches.
    let profile = fixture_profile("fridge-tls.zip");
    let model = profile.model();
    let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());

    let mut checked = 0_usize;
    for entry in mapping.entries() {
        let surfaced = entry.state_topic.is_some() || entry.command_topic.is_some();
        let boolean = model
            .entity(entry.uid)
            .and_then(hc_profile::Entity::protocol_type)
            == Some(hc_profile::ProtocolType::Boolean);
        // A `<command>` is boolean too but is a button, and an event is separate; only value-carrying
        // kinds are under test.
        let value_kind = model.entity(entry.uid).is_some_and(|entity| {
            matches!(
                entity.kind,
                hc_profile::EntityKind::Setting
                    | hc_profile::EntityKind::Status
                    | hc_profile::EntityKind::Option
            )
        });
        if !surfaced || !boolean || !value_kind {
            continue;
        }
        let writable = entry.command_topic.is_some();
        let expected = if writable {
            EntityClass::Switch
        } else {
            EntityClass::BinarySensor
        };
        assert_eq!(
            entry.class, expected,
            "{:?}: a boolean is a switch when writable and a binary sensor \
             when not — never the generic sensor fallback",
            entry.key
        );
        checked += 1;
    }
    assert!(
        checked > 0,
        "the fridge declares no surfaced booleans, so this test proved nothing"
    );
}

#[tokio::test]
async fn the_device_is_named_for_the_appliance_when_it_knows_what_it_is() {
    // HA composes every `friendly_name` from the device name, so it is "<brand> - <model>" from the
    // caller's `DeviceInfo`, never hardcoded. `identifiers` stay the slug: HA keys the device
    // registry on them, and changing them would orphan every entity and automation.
    let info = hc_core::discovery::DeviceInfo {
        configuration_url: None,
        brand: Some("SIEMENS".to_owned()),
        model: Some("KG49NSBBF".to_owned()),
        sw_version: None,
    };
    let profile = fixture_profile("fridge-tls.zip");
    let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
    let bridge = DeviceBridge::new(BASE, device_id());
    let mut broker = FakeBroker::new();
    bridge
        .publish_discovery(
            &mut broker,
            &mapping,
            &Announcement {
                mode: DiscoveryMode::Full,
                info: &info,
                names: &hc_core::discovery::DerivedNames,
                origin: &hc_core::discovery::DiscoveryOrigin::KESSEL,
            },
        )
        .await
        .expect("publish discovery");

    let published = broker
        .discovery_publishes()
        .next()
        .expect("the fridge publishes discovery configs");
    let payload: Value =
        serde_json::from_slice(&published.payload).expect("a discovery payload is JSON");
    let device = device_block_of(&payload, "fridge-tls.zip", &published.topic);
    assert_eq!(
        device.get("name"),
        Some(&Value::String("SIEMENS - KG49NSBBF".to_owned())),
        "the device is named for the appliance it is, composed from the identity it was given"
    );
    assert_eq!(
        device.get("identifiers"),
        Some(&Value::Array(vec![Value::String(format!(
            "kessel_{}",
            device_id().as_str()
        ))])),
        "…and the registry key stays the slug, so renaming an appliance is not a new device"
    );
}

#[tokio::test]
async fn a_device_with_no_identity_is_still_named_by_its_slug() {
    // With no identity supplied the device is still named, because HA shows a blank name as an
    // unusable row.
    let broker = discovery_broker("fridge-tls.zip").await;
    let published = broker
        .discovery_publishes()
        .next()
        .expect("the fridge publishes discovery configs");
    let payload: Value =
        serde_json::from_slice(&published.payload).expect("a discovery payload is JSON");
    let device = device_block_of(&payload, "fridge-tls.zip", &published.topic);
    assert_eq!(
        device.get("name"),
        Some(&Value::String(device_id().as_str().to_owned())),
        "with no brand and no model, the operator's own slug is the only name there is"
    );
}

#[tokio::test]
async fn the_selected_program_is_a_select_offering_the_appliances_own_programmes() {
    // `selectedProgram` is a `select` whose `options` are the appliance's own programme keys (empty
    // on the fridge), uncategorised. Those keys are also what `publish_value` puts on the state
    // topic, so the round trip closes.
    for (fixture, expect_programmes) in [("oven-tls.zip", true), ("fridge-tls.zip", false)] {
        let profile = fixture_profile(fixture);
        let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
        let entry = program_pointer(&mapping, "BSH.Common.Root.SelectedProgram");
        assert_eq!(
            entry.class,
            EntityClass::Select,
            "{fixture}: a writable programme pointer is a select, not an anonymous sensor"
        );
        let declared: Vec<String> = mapping
            .programs()
            .iter()
            .filter_map(|program| program.key.clone())
            .collect();
        assert_eq!(
            entry.options, declared,
            "{fixture}: the options are the appliance's own declared programmes, in its own order"
        );
        assert_eq!(
            !entry.options.is_empty(),
            expect_programmes,
            "{fixture}: the oven declares programmes and the fridge does not — an empty list is a \
             legitimate answer, not a missing one"
        );
    }
}

#[tokio::test]
async fn choosing_a_programme_builds_the_evidenced_program_frame_not_a_values_write() {
    // A write path: on the oven and the hob, `/ro/selectedProgram` is a program start. Built here,
    // not transmitted. The programme select must not take the `/ro/values` path; the frame
    // addresses the programme by uid, so `target_uid` carries it.
    let profile = fixture_profile("oven-tls.zip");
    let model = profile.model();
    let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());
    let entry = program_pointer(&mapping, "BSH.Common.Root.SelectedProgram");
    let programme = mapping
        .programs()
        .iter()
        .find(|program| program.key.is_some())
        .expect("the oven declares named programmes");
    let chosen = programme.key.clone().expect("checked above");

    let command = hc_core::bridge::build_write_command(
        model,
        &mapping,
        entry.command_topic.as_deref().expect("it is writable"),
        &chosen,
    )
    .expect("a declared programme name is a valid selection");

    assert_eq!(
        command.resource, "/ro/selectedProgram",
        "a programme selection takes the evidenced programme frame"
    );
    assert_eq!(
        command.target_uid,
        Some(programme.uid),
        "…addressing the PROGRAMME's uid, not the pointer feature's"
    );
}

#[tokio::test]
async fn a_programme_this_appliance_does_not_declare_is_refused_locally() {
    // An option HA was never offered must not reach the appliance; the select's `options` are built
    // from the same list, so no capability is lost.
    let profile = fixture_profile("oven-tls.zip");
    let model = profile.model();
    let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());
    let entry = program_pointer(&mapping, "BSH.Common.Root.SelectedProgram");
    let topic = entry.command_topic.as_deref().expect("it is writable");

    for refused in ["Cooking.Oven.Program.NoSuchProgramme", "", "8208"] {
        assert!(
            hc_core::bridge::build_write_command(model, &mapping, topic, refused).is_err(),
            "{refused:?} is not a programme this appliance declares and must be refused before the \
             guard, not written"
        );
    }
}

#[tokio::test]
async fn an_appliance_that_runs_programmes_gets_a_stop_button() {
    // A stop button is gated on the programme pointer existing, not on a non-empty programme list:
    // the appliance may run something the profile does not name. There is no start button; the
    // programme `select` already starts.
    for fixture in FIXTURES {
        let profile = fixture_profile(fixture);
        let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
        let has_pointer = mapping.entries().iter().any(|entry| {
            matches!(
                entry.key.as_deref(),
                Some("BSH.Common.Root.ActiveProgram" | "BSH.Common.Root.SelectedProgram")
            )
        });
        // `announce`, not `publish_discovery`: only `announce` subscribes.
        let bridge = DeviceBridge::new(BASE, device_id());
        let mut broker = FakeBroker::new();
        bridge
            .announce(
                &mut broker,
                &mapping,
                &Announcement::bare(DiscoveryMode::Full),
            )
            .await
            .expect("announce");
        let stop = broker
            .discovery_publishes()
            .find(|published| published.topic.ends_with("/stop_program/config"));
        assert_eq!(
            stop.is_some(),
            has_pointer,
            "{fixture}: a stop button belongs exactly to an appliance that runs programmes"
        );
        let Some(stop) = stop else { continue };
        let payload: Value =
            serde_json::from_slice(&stop.payload).expect("a discovery payload is JSON");
        assert!(
            stop.topic.starts_with("homeassistant/button/"),
            "{fixture}: {}: a stop control is a button",
            stop.topic
        );
        assert_eq!(
            payload.get("command_topic"),
            Some(&Value::String(format!(
                "{BASE}/{}/_control/stop_program",
                device_id().as_str()
            ))),
            "{fixture}: the control topic backs no feature, so it lives under `_control/`"
        );
        assert_eq!(
            payload.get("entity_category"),
            None,
            "{fixture}: stopping a running appliance is not a diagnostic"
        );
        assert!(
            broker.subscriptions.iter().any(|filter| filter
                == &format!("{BASE}/{}/_control/stop_program", device_id().as_str())),
            "{fixture}: a button nobody is subscribed to is a button that does nothing"
        );
    }
}

#[tokio::test]
async fn pressing_stop_builds_a_delete_of_the_active_program() {
    // A write path: it aborts whatever the appliance is running. Built here, not transmitted.
    //
    // The frame is `DELETE /ro/activeProgram` with no body. Asserted down to the emitted frame,
    // because a new resource must not reach the wire without a known frame behind it.
    let profile = fixture_profile("oven-tls.zip");
    let model = profile.model();
    let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());

    let command =
        hc_core::bridge::build_write_command(model, &mapping, "_control/stop_program", "")
            .expect("the stop control is routable");
    assert_eq!(command.resource, "/ro/activeProgram");
    assert_eq!(
        command.target_uid, None,
        "clearing the active programme addresses no uid — the resource is the whole address"
    );

    let frame = hc_core::bridge::to_outgoing(&command, 7, 9, hc_proto::Version::Int(1))
        .expect("the stop frame is evidenced and must build");
    assert_eq!(frame.action, hc_proto::Action::Delete);
    assert_eq!(frame.resource, "/ro/activeProgram");
    assert_eq!(
        frame.body,
        hc_proto::Body::Empty,
        "a DELETE carries no `data` key at all — `Body::Empty` omits it rather than emitting \
         `null` or an unattested `[]` (`message-model.md` §2.3)"
    );
}
