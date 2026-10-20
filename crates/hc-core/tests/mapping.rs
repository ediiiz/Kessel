//! The generic feature-to-topic/entity mapping, over four captured device descriptions (the fridge
//! and the oven bracket the range: few features vs. many).
//!
//! The exhaustiveness tests count readable/writable features in the source
//! [`hc_profile::EntityModel`] independently of the mapping, then assert the mapping matches.

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

use hc_core::mapping::{
    DeviceClass, DeviceId, DeviceMapping, EntityClass, EntityMapping, Excludes, LiveDescriptions,
    feature_path, normalize_program_name,
};
use hc_profile::{EntityModel, NumericRange, Profile};

/// The four scrubbed fixtures in `hc-profile`'s test tree, read-only. This crate consumes the merged
/// model and never parses XML itself.
const FIXTURES: [&str; 4] = [
    "hob-aes.zip",
    "fridge-tls.zip",
    "dishwasher-tls.zip",
    "oven-tls.zip",
];

/// `<workspace>/crates/hc-profile/tests/fixtures/<name>`, reached relative to this crate's dir.
fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("hc-profile")
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// A committed fixture, parsed into its [`Profile`].
fn fixture_profile(name: &str) -> Profile {
    let bytes = std::fs::read(fixture_path(name))
        .unwrap_or_else(|err| panic!("reading fixture {name}: {err}"));
    Profile::from_zip_bytes(&bytes).unwrap_or_else(|err| panic!("parsing fixture {name}: {err}"))
}

/// Whether a kind of node carries a value, listed positively.
///
/// Deliberately independent of `hc_core`'s own `carries_value`: a helper that delegated to the code
/// under test would agree with any bug in it.
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

/// Counts value-bearing features in the source model, independently of the mapping.
///
/// Not `Access::is_readable`: the fridge declares `access="none"` on its settings and still reports
/// values for them, while its `settingList` containers declare `readWrite` and report nothing.
fn value_bearing_count(model: &EntityModel) -> usize {
    model
        .entities()
        .values()
        .filter(|entity| is_value_bearing(entity.kind))
        .count()
}

/// Whether a kind of node can be an entity at all: value-bearing, or a `<command>` button.
///
/// Treating `access="none"` as writable is gated on this: the hob declares `access="none"` on its
/// `<protectionPort>`, which would otherwise become a button on a cooktop.
const fn is_addressable(kind: hc_profile::EntityKind) -> bool {
    is_value_bearing(kind) || matches!(kind, hc_profile::EntityKind::Command)
}

/// Whether a feature takes a `…/set` topic, stated independently of the mapping.
///
/// Writable means `readWrite`/`writeOnly`, or `access="none"` on an entity-capable node.
/// `read`/`readStatic`, an absent attribute and an unrecognised word stay out (see
/// `hc_core::write::check_access`).
fn takes_a_set_topic(entity: &hc_profile::Entity) -> bool {
    match entity.access {
        Some(hc_profile::Access::None) => is_addressable(entity.kind),
        ref access => access.as_ref().is_some_and(hc_profile::Access::is_writable),
    }
}

/// Counts writable features in the source model, independently of the mapping.
fn writable_count(model: &EntityModel) -> usize {
    model
        .entities()
        .values()
        .filter(|entity| takes_a_set_topic(entity))
        .count()
}

/// A synthetic device id (never a real haId/serial/mac).
fn device_id() -> DeviceId {
    DeviceId::new("test-device")
}

/// Fridge settings declared `access="none"` still get state topics.
///
/// The fridge declares `access="none" available="false"` on all its settings and reports values
/// for them anyway, so `access` is not an existence test. A value with no state topic is dropped
/// by `bridge::publish_value`.
const FRIDGE_REPORTED_BUT_ACCESS_NONE: [&str; 6] = [
    "BSH.Common.Setting.PowerState",
    "Refrigeration.Common.Setting.Freezer.SetpointTemperature",
    "Refrigeration.Common.Setting.Refrigerator.SetpointTemperature",
    "Refrigeration.Common.Setting.EcoMode",
    "Refrigeration.Common.Setting.Light.Internal.Power",
    "Refrigeration.Common.Setting.Light.External.Power",
];

#[test]
fn a_reported_feature_has_a_state_topic_whatever_its_access_says() {
    let profile = fixture_profile("fridge-tls.zip");
    let model = profile.model();
    let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());

    for key in FRIDGE_REPORTED_BUT_ACCESS_NONE {
        let entry = entry_for(&mapping, key);
        // Pin the premise: if a fixture re-scrub changed the access word, this would pass for the
        // wrong reason.
        let entity = model
            .entity(entry.uid)
            .unwrap_or_else(|| panic!("{key}: fixture entity"));
        assert_eq!(
            entity.access,
            Some(hc_profile::Access::None),
            "{key}: premise — the fridge declares this `access=\"none\"`"
        );
        assert_eq!(
            entry.state_topic.as_deref(),
            Some(format!("{}/state", feature_path(key)).as_str()),
            "{key}: the fridge reports a value for this feature, so it must have a \
             state topic to report it on"
        );
        // `access="none"` is the appliance declining to answer, not saying "no", so the write is
        // attempted and the appliance's own error response is the authority.
        assert_eq!(
            entry.command_topic.as_deref(),
            Some(format!("{}/set", feature_path(key)).as_str()),
            "{key}: `access=\"none\"` is attempted, not refused locally"
        );
    }
}

#[test]
fn a_read_only_feature_is_still_never_writable() {
    // `access="read"` is a positive "no" and gets no `…/set` topic. Asserted over the whole fleet
    // so a future widening of `is_writable` cannot take it along.
    for name in FIXTURES {
        let profile = fixture_profile(name);
        let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
        for entry in mapping.entries() {
            let Some(entity) = profile.model().entity(entry.uid) else {
                continue;
            };
            if !matches!(
                entity.access,
                Some(hc_profile::Access::Read | hc_profile::Access::ReadStatic)
            ) {
                continue;
            }
            assert!(
                entry.command_topic.is_none(),
                "{name}: {:?} declares itself read-only and must have no `…/set` topic",
                entry.key
            );
        }
    }
}

#[test]
fn state_topic_count_equals_value_bearing_feature_count() {
    // Count value-bearing features in the source model, then assert the mapping produced exactly
    // that many state topics, for each of the four real descriptions.
    for name in FIXTURES {
        let profile = fixture_profile(name);
        let model = profile.model();
        let expected = value_bearing_count(model);
        assert!(
            expected > 0,
            "{name}: fixture should have value-bearing features"
        );

        let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());
        let produced = mapping.state_topics().count();

        assert_eq!(
            produced, expected,
            "{name}: every value-bearing feature must produce exactly one state topic \
             (produced {produced}, model has {expected} value-bearing)"
        );
    }
}

/// The full set of topics (state + command) a mapping produces, as an ownable set.
fn topic_set(mapping: &DeviceMapping) -> BTreeSet<String> {
    mapping
        .state_topics()
        .chain(mapping.command_topics())
        .map(str::to_owned)
        .collect()
}

#[test]
fn exclude_is_the_only_omission_mechanism() {
    // Excluding a readWrite feature removes exactly its state and set topics and nothing else; the
    // exclude list is the only thing that drops a feature.
    for name in FIXTURES {
        let profile = fixture_profile(name);
        let model = profile.model();

        // A feature that is both readable and writable, so its omission must move two topics.
        let victim = model
            .entities()
            .values()
            .find(|entity| {
                entity.key.is_some()
                    && entity
                        .access
                        .as_ref()
                        .is_some_and(hc_profile::Access::is_writable)
                    && entity
                        .access
                        .as_ref()
                        .is_some_and(hc_profile::Access::is_readable)
            })
            .and_then(|entity| entity.key.clone())
            .unwrap_or_else(|| panic!("{name}: expected a readWrite feature"));

        let full = DeviceMapping::build(&device_id(), model, &Excludes::none());
        let full_topics = topic_set(&full);

        let path = feature_path(&victim);
        let victim_topics: BTreeSet<String> =
            [format!("{path}/state"), format!("{path}/set")].into();
        assert!(
            victim_topics.is_subset(&full_topics),
            "{name}: the victim's state+set topics must be present without an exclude"
        );

        let pruned = DeviceMapping::build(&device_id(), model, &Excludes::of([victim.clone()]));
        let pruned_topics = topic_set(&pruned);

        let removed: BTreeSet<String> = full_topics.difference(&pruned_topics).cloned().collect();
        assert_eq!(
            removed, victim_topics,
            "{name}: excluding {victim} must remove EXACTLY its state+set topics, nothing else"
        );
        assert!(
            pruned_topics.is_subset(&full_topics),
            "{name}: excluding a feature must not ADD or change any other topic"
        );
        assert!(
            pruned
                .entries()
                .iter()
                .all(|e| e.key.as_deref() != Some(&victim)),
            "{name}: the excluded feature must have no entry at all"
        );
    }
}

#[test]
fn every_writable_feature_has_set_topic() {
    // Every writable feature in the source model gets exactly one command topic, on its own entry.
    for name in FIXTURES {
        let profile = fixture_profile(name);
        let model = profile.model();
        let expected = writable_count(model);
        assert!(
            expected > 0,
            "{name}: fixture should have writable features"
        );

        let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());
        assert_eq!(
            mapping.command_topics().count(),
            expected,
            "{name}: every writable feature must produce exactly one /set topic"
        );

        for entry in mapping.entries() {
            let writable = model.entity(entry.uid).is_some_and(takes_a_set_topic);
            assert_eq!(
                entry.command_topic.is_some(),
                writable,
                "{name}: uid {:#06X} command topic presence must match writability",
                entry.uid
            );
        }
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

#[test]
fn a_string_or_object_feature_is_never_a_number() {
    // A declared `min`/`max` is a constraint, not a type. For two families of real features it
    // bounds something that is not a number:
    //
    //   BSH.Common.Setting.Favorite.001.Name     refCID=05 → String  (min/max = a length, 0..30)
    //   BSH.Common.Setting.Favorite.001.Program  refCID=A8 → Object  (a program + its option list)
    //
    // `refCID` gives the data type (1→Boolean, 2,3→Integer, 4→Float, 5→String, 129..194→Object).
    // HA rejects a non-numeric payload on a `number` entity.
    let dishwasher = fixture_profile("dishwasher-tls.zip");
    let dw = DeviceMapping::build(&device_id(), dishwasher.model(), &Excludes::none());

    for (key, why) in [
        (
            "BSH.Common.Setting.Favorite.001.Name",
            "refCID 05 is a String; 0..30 bounds its LENGTH",
        ),
        (
            "BSH.Common.Setting.Favorite.001.Program",
            "refCID A8 is an Object; it reports {\"length\":0,\"list\":[]}",
        ),
    ] {
        let entry = entry_for(&dw, key);
        assert_ne!(
            entry.class,
            EntityClass::Number,
            "{key} must not be a number — {why}"
        );
        assert_eq!(
            entry.class,
            EntityClass::Sensor,
            "{key} falls to the generic sensor, the class that publishes a value verbatim"
        );
        assert_eq!(
            entry.number_range, None,
            "{key} must carry no min/max into the discovery config either"
        );
    }
}

#[test]
fn only_a_boolean_becomes_a_writable_switch() {
    // A writable, unconstrained Setting/Option is not assumed boolean when `refCID` says otherwise:
    //
    //   BSH.Common.Setting.Timezone             refCID=31 → Object  (content type 49, timezone)
    //   BSH.Common.Setting.ApplianceDateTime    refCID=06 → String  (content type 6, dateTime)
    //   BSH.Common.Option.BaseProgram           refCID=15 → Integer (content type 21, uidValue)
    //
    // A `switch` would write `true`/`false` to a timezone, a timestamp or a program uid. Anything
    // whose type is known and is not boolean falls back to a read-only sensor.
    for (fixture, key, why) in [
        (
            "dishwasher-tls.zip",
            "BSH.Common.Setting.Timezone",
            "refCID 31 is an Object",
        ),
        (
            "dishwasher-tls.zip",
            "BSH.Common.Setting.ApplianceDateTime",
            "refCID 06 is a String",
        ),
        (
            "oven-tls.zip",
            "BSH.Common.Setting.Timezone",
            "refCID 31 is an Object on the oven too",
        ),
    ] {
        let profile = fixture_profile(fixture);
        let mapping = DeviceMapping::build(&device_id(), profile.model(), &Excludes::none());
        let entry = entry_for(&mapping, key);
        assert_ne!(
            entry.class,
            EntityClass::Switch,
            "{fixture}: {key} must not be a writable switch — {why}"
        );
        // The raw `…/set` topic stays in the mapping; `a_sensor_config_offers_no_command_topic`
        // checks that discovery offers HA no control.
    }

    // A real boolean setting is still a switch, so the class was not simply emptied.
    let hob = fixture_profile("hob-aes.zip");
    let hb = DeviceMapping::build(&device_id(), hob.model(), &Excludes::none());
    assert!(
        hb.entries()
            .iter()
            .any(|entry| entry.class == EntityClass::Switch),
        "the hob must still have boolean switches"
    );
}

#[test]
fn writable_number_takes_min_max_step_from_the_entity() {
    // A number's min/max/step come from the entity, not a hard-coded default: two real numerics
    // with different bounds could not both satisfy one fixed range.
    let dishwasher = fixture_profile("dishwasher-tls.zip");
    let dw = DeviceMapping::build(&device_id(), dishwasher.model(), &Excludes::none());

    let silence = entry_for(
        &dw,
        "Dishcare.Dishwasher.Setting.SilenceOnDemandDefaultTime",
    );
    assert_eq!(
        silence.class,
        EntityClass::Number,
        "a writable numeric is a number"
    );
    assert_eq!(
        silence.number_range,
        Some(NumericRange {
            min: Some(60),
            max: Some(1800),
            step: Some(60),
        }),
        "SilenceOnDemandDefaultTime bounds are taken from the entity"
    );

    // The second numeric comes from a different appliance, so the two bounds come from two
    // separate descriptions.
    let hob = fixture_profile("hob-aes.zip");
    let hb = DeviceMapping::build(&device_id(), hob.model(), &Excludes::none());
    let alarm = entry_for(&hb, "BSH.Common.Setting.AlarmClock");
    assert_eq!(
        alarm.class,
        EntityClass::Number,
        "a writable numeric is a number"
    );
    assert_eq!(
        alarm.number_range,
        Some(NumericRange {
            min: Some(0),
            max: Some(5940),
            step: None,
        }),
        "a different numeric yields its own, different bounds — proving nothing is hard-coded"
    );

    assert_ne!(
        silence.number_range, alarm.number_range,
        "two numerics must not share one hard-coded range"
    );
}

#[test]
fn entity_class_over_real_features() {
    // One assertion per entity class, over real captured features.
    let dishwasher = fixture_profile("dishwasher-tls.zip");
    let dw = DeviceMapping::build(&device_id(), dishwasher.model(), &Excludes::none());
    let fridge = fixture_profile("fridge-tls.zip");
    let fr = DeviceMapping::build(&device_id(), fridge.model(), &Excludes::none());

    // switch — a writable boolean Setting (no enum, no range).
    assert_eq!(
        entry_for(&dw, "Dishcare.Dishwasher.Setting.ExtraDry").class,
        EntityClass::Switch,
        "a writable boolean Setting is a switch"
    );

    // select — a writable enumeration, carrying its member names as options.
    let power = entry_for(&dw, "BSH.Common.Setting.PowerState");
    assert_eq!(
        power.class,
        EntityClass::Select,
        "a writable enum is a select"
    );
    // The DeviceDescription narrows the enum to the values this appliance supports (Off, On); the
    // FeatureMapping names all four.
    assert_eq!(
        power.options,
        ["Off", "On"],
        "select options are the resolved, declared enum member names"
    );

    // number — a writable numeric.
    assert_eq!(
        entry_for(
            &dw,
            "Dishcare.Dishwasher.Setting.SilenceOnDemandDefaultTime"
        )
        .class,
        EntityClass::Number,
        "a writable numeric is a number"
    );

    // sensor — a readable value with no more specific class; an enum sensor carries device_class ENUM.
    let door_state = entry_for(&fr, "BSH.Common.Status.DoorState");
    assert_eq!(
        door_state.class,
        EntityClass::Sensor,
        "a readable four-state enum is a sensor"
    );
    assert_eq!(
        door_state.device_class,
        Some(DeviceClass::Enum),
        "an enum sensor carries device_class ENUM"
    );
    assert_eq!(
        entry_for(&fr, "BSH.Common.Status.WiFiSignalStrength").class,
        EntityClass::Sensor,
        "a readable non-enum value is a sensor"
    );

    // binary_sensor: an enumeration whose declared members are literally Off/On.
    //
    // A two-member door enum (`Open`/`Closed`) is not one: nothing says which member is `on`, and
    // the published label would match neither HA payload, so it would read `unknown` forever.
    let door = entry_for(&fr, "Refrigeration.Common.Status.Door.Refrigerator");
    assert_eq!(
        door.class,
        EntityClass::Sensor,
        "a Closed/Open door is not an on/off set: it is an enum sensor, which HA can render, \
         rather than a binary sensor that can never match its own state"
    );
    assert_eq!(
        door.options,
        ["Closed", "Open"],
        "and it keeps its declared members, so HA shows the door's real state"
    );
    // The hob, because only the hob narrows this key to Off/On: the fridge declares four states (an
    // enum sensor) and the dishwasher makes it writable (a select). The class comes from what this
    // appliance declares, not from a per-key table.
    let hob = fixture_profile("hob-aes.zip");
    let hb = DeviceMapping::build(&device_id(), hob.model(), &Excludes::none());
    let power_off_on = entry_for(&hb, "BSH.Common.Setting.PowerState");
    assert_eq!(
        power_off_on.options,
        ["Off", "On"],
        "the hob narrows PowerState to exactly the on/off pair"
    );
    assert_eq!(
        power_off_on.class,
        EntityClass::BinarySensor,
        "an Off/On enumeration IS an on/off set — the one shape a binary sensor can represent"
    );

    // button — a Command.
    assert_eq!(
        entry_for(&fr, "BSH.Common.Command.AcknowledgeEvent").class,
        EntityClass::Button,
        "a Command is a button"
    );
}

#[test]
fn program_names_normalized() {
    // Program names are normalised (lower-cased, '.' -> '_'), and every named program in the source
    // model is exposed. The fridge has none, which is an empty list, not an error.
    assert_eq!(
        normalize_program_name("Dishcare.Dishwasher.Program.Eco50"),
        "dishcare_dishwasher_program_eco50",
        "lower-cased and '.'->'_'"
    );

    for name in FIXTURES {
        let profile = fixture_profile(name);
        let model = profile.model();
        let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());

        let source_named = model
            .programs()
            .values()
            .filter(|p| p.key.is_some())
            .count();
        let mapped_named = mapping
            .programs()
            .iter()
            .filter(|p| p.normalized_name.is_some())
            .count();
        assert_eq!(
            mapped_named, source_named,
            "{name}: every named program in the model must appear with a normalised name"
        );

        for program in mapping.programs() {
            if let Some(key) = program.key.as_deref() {
                assert_eq!(
                    program.normalized_name.as_deref(),
                    Some(normalize_program_name(key).as_str()),
                    "{name}: program {key} name must be normalised"
                );
            }
        }
    }
}

#[test]
fn unique_id_is_deviceid_plus_feature_key() {
    // unique_id = "<device_id>-<feature_key>": stable and collision-free within a device. The device
    // component is the caller-supplied DeviceId, never the haId/serial/mac.
    let device = DeviceId::new("kitchen-hob");
    for name in FIXTURES {
        let profile = fixture_profile(name);
        let model = profile.model();
        let mapping = DeviceMapping::build(&device, model, &Excludes::none());

        let mut seen = BTreeSet::new();
        for entry in mapping.entries() {
            if let Some(key) = entry.key.as_deref() {
                assert_eq!(
                    entry.unique_id,
                    format!("{device}-{key}"),
                    "{name}: unique_id must be <device_id>-<feature_key>"
                );
            }
            assert!(
                seen.insert(entry.unique_id.clone()),
                "{name}: unique_id {} collides within one device",
                entry.unique_id
            );
        }
    }
}

#[test]
fn topic_path_is_pure_dotted_to_slash_transform() {
    // The topic path is the feature key with '.' -> '/': "<path>/state" for readable entities,
    // and likewise for command topics on writable ones.
    for name in FIXTURES {
        let profile = fixture_profile(name);
        let model = profile.model();
        let mapping = DeviceMapping::build(&device_id(), model, &Excludes::none());

        let mut checked = 0_usize;
        for entry in mapping.entries() {
            let Some(key) = entry.key.as_deref() else {
                continue;
            };
            let expected_path = feature_path(key);
            if let Some(state) = entry.state_topic.as_deref() {
                assert_eq!(
                    state,
                    format!("{expected_path}/state"),
                    "{name}: state topic for {key} must be the pure dotted->slash transform"
                );
                checked += 1;
            }
            if let Some(command) = entry.command_topic.as_deref() {
                assert_eq!(
                    command,
                    format!("{expected_path}/set"),
                    "{name}: command topic for {key} must be the pure dotted->slash transform"
                );
            }
        }
        assert!(checked > 0, "{name}: expected some named readable features");
    }
}

/// A `NOTIFY /ro/descriptionChange` overrides the archive's description, because it is newer.
///
/// The fridge's archive declares `min="0" max="0" stepSize="0"` on its temperature setpoints; a
/// live update with real bounds replaces that. The merge is sparse: a field the update does not
/// mention keeps the archive's value.
#[test]
fn a_live_description_overrides_the_archive_it_is_newer_than() {
    const KEY: &str = "Refrigeration.Common.Setting.Freezer.SetpointTemperature";

    let profile = fixture_profile("fridge-tls.zip");
    let model = profile.model();
    let uid = model
        .entities()
        .values()
        .find(|entity| entity.key.as_deref() == Some(KEY))
        .map(|entity| entity.uid)
        .expect("the fridge declares its freezer setpoint");

    // Premise: from the archive alone this is a control with no bounds anyone stated.
    let archived = DeviceMapping::build(&device_id(), model, &Excludes::none());
    let archived = entry_for(&archived, KEY);
    assert_eq!(archived.class, EntityClass::Number);
    assert_eq!(
        archived.number_range, None,
        "premise: `min=0 max=0 stepSize=0` is not a range a control can be offered over"
    );

    // What a `NOTIFY /ro/descriptionChange` for this uid would carry.
    let mut live = LiveDescriptions::default();
    assert!(
        live.absorb(&hc_proto::ValueUpdate {
            uid: u64::from(uid),
            value: Some(serde_json::Value::from(-18)),
            description: hc_proto::DescriptionUpdate {
                access: Some("readWrite".to_owned()),
                available: Some(true),
                min: Some(-24),
                max: Some(-16),
                step: Some(1),
            },
        }),
        "absorbing a description the overlay did not have is a change"
    );

    let described = DeviceMapping::build_with(&device_id(), model, &Excludes::none(), &live);
    let described = entry_for(&described, KEY);
    assert_eq!(
        described.number_range,
        Some(NumericRange {
            min: Some(-24),
            max: Some(-16),
            step: Some(1),
        }),
        "the bounds the appliance stated at run time are the ones the control gets"
    );
    assert_eq!(described.class, EntityClass::Number);

    // Sparse: a plain value update mentions nothing, so it changes nothing.
    assert!(
        !live.absorb(&hc_proto::ValueUpdate {
            uid: u64::from(uid),
            value: Some(serde_json::Value::from(-19)),
            description: hc_proto::DescriptionUpdate::default(),
        }),
        "a value-only item revises no description and must not be reported as a change"
    );
    let again = DeviceMapping::build_with(&device_id(), model, &Excludes::none(), &live);
    assert_eq!(
        entry_for(&again, KEY).number_range,
        described.number_range,
        "and it must not erase the bounds a previous description change established"
    );
}

/// A control the appliance locks for a while (`access` revised to `read` while a programme runs,
/// say) keeps its platform and its command topic. Moving it to `sensor` would delete the Home
/// Assistant entity and every automation that refers to it; the appliance refuses the write
/// instead, and the state snaps back.
#[test]
fn a_control_locked_at_run_time_keeps_its_shape() {
    const KEY: &str = "Refrigeration.Common.Setting.Freezer.SetpointTemperature";

    let profile = fixture_profile("fridge-tls.zip");
    let model = profile.model();
    let uid = model
        .entities()
        .values()
        .find(|entity| entity.key.as_deref() == Some(KEY))
        .map(|entity| entity.uid)
        .expect("the fridge declares its freezer setpoint");
    let archived = DeviceMapping::build(&device_id(), model, &Excludes::none());
    let archived = entry_for(&archived, KEY).clone();
    assert_eq!(archived.class, EntityClass::Number, "premise: a control");

    let mut live = LiveDescriptions::default();
    live.absorb(&hc_proto::ValueUpdate {
        uid: u64::from(uid),
        value: Some(serde_json::Value::Null),
        description: hc_proto::DescriptionUpdate {
            access: Some("read".to_owned()),
            ..hc_proto::DescriptionUpdate::default()
        },
    });

    let locked = DeviceMapping::build_with(&device_id(), model, &Excludes::none(), &live);
    let locked = entry_for(&locked, KEY);
    assert_eq!(locked.class, archived.class, "the platform does not move");
    assert_eq!(locked.unique_id, archived.unique_id);
    assert_eq!(
        locked.command_topic, archived.command_topic,
        "the command topic stays, so the subscription and the entity survive"
    );
}
