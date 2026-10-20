//! Profile parsing tests.
//!
//! Every test runs against the committed, scrubbed fixtures. Some additionally run against the
//! real ZIPs when `KESSEL_RAW_PROFILES` is set, and announce a skip when it is not.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests may panic; library code may not"
)]

mod support;

use std::time::{Duration, Instant};

use hc_profile::{
    Access, ConnectionType, EntityKind, EntityModel, Profile, ProfileError, SkipReason, ValueType,
};
use support::{
    FIXTURES, Fixture, build_deflated_zip, build_zip, fixture_bytes, fixture_profile,
    for_each_raw_profile, has_extension, read_zip,
};

// ---------------------------------------------------------------------------------------------
// The two XML members are found through the JSON, not through the filename convention.
// ---------------------------------------------------------------------------------------------

#[test]
fn parses_zip_via_json_declared_filenames() {
    let members = read_zip(&fixture_bytes("fridge-tls.zip"));
    let manifest_body = members
        .iter()
        .find(|(name, _)| has_extension(name, "json"))
        .map(|(_, body)| body.clone())
        .expect("the fixture has a manifest");
    let mut manifest: serde_json::Value = serde_json::from_slice(&manifest_body).unwrap();

    // Names that the `<serial>_DeviceDescription.xml` convention could never guess, and that do
    // not even carry an `.xml` extension.
    let description = members
        .iter()
        .find(|(name, _)| name.ends_with("_DeviceDescription.xml"))
        .map(|(_, body)| body.clone())
        .unwrap();
    let mapping = members
        .iter()
        .find(|(name, _)| name.ends_with("_FeatureMapping.xml"))
        .map(|(_, body)| body.clone())
        .unwrap();
    let fields = manifest
        .as_object_mut()
        .expect("the manifest is a JSON object");
    fields.insert(
        "deviceDescriptionFileName".to_owned(),
        serde_json::json!("payload-a.bin"),
    );
    fields.insert(
        "featureMappingFileName".to_owned(),
        serde_json::json!("payload-b.bin"),
    );

    let forged = build_zip(&[
        ("some-manifest.json".to_owned(), manifest.to_string().into()),
        ("payload-a.bin".to_owned(), description),
        ("payload-b.bin".to_owned(), mapping),
    ]);

    let by_declaration = Profile::from_zip_bytes(&forged)
        .expect("the declared filenames are what the parser must follow");
    let by_convention = fixture_profile("fridge-tls.zip");

    assert_eq!(
        by_declaration.model().entities().len(),
        by_convention.model().entities().len(),
        "renaming the members but keeping the declaration honest must change nothing"
    );
    assert!(
        !by_declaration.model().entities().is_empty(),
        "the comparison above is only meaningful on a non-empty model"
    );
    assert_eq!(
        by_declaration
            .model()
            .entity_by_key("BSH.Common.Status.DoorState")
            .map(|entity| entity.uid),
        Some(0x020F),
        "the FeatureMapping was read from `payload-b.bin` as declared"
    );
}

// ---------------------------------------------------------------------------------------------
// psk64 = key; AES also carries iv64. Both are wrapped at the parse boundary.
// ---------------------------------------------------------------------------------------------

#[test]
fn assigns_psk_and_iv_for_aes_profile() {
    let profile = fixture_profile("hob-aes.zip");
    let credentials = profile.credentials();

    assert_eq!(
        profile.identity().connection_type,
        ConnectionType::Aes,
        "the hob is the fleet's only AES device"
    );
    assert_eq!(
        credentials.psk64.expose_secret().as_str(),
        "DNseq29EsGnxeEDq0et54_tIpN2KZqePkTEAovtYe9m",
        "psk64 comes from the manifest's `key`"
    );
    assert_eq!(
        credentials
            .iv64
            .as_ref()
            .map(|iv| iv.expose_secret().as_str()),
        Some("kVqFvXYNjIzqe2gQ_JvEaA"),
        "AES carries an iv64"
    );

    // The wrapping is what makes a stray `{:?}` harmless.
    let rendered = format!("{credentials:?}");
    assert!(
        rendered.contains("redacted"),
        "Credentials must render redacted, got {rendered}"
    );
    assert!(
        !rendered.contains("DNseq"),
        "the psk must not survive a Debug format"
    );
    assert!(
        !rendered.contains("kVqFv"),
        "the iv must not survive a Debug format"
    );
}

#[test]
fn assigns_psk_only_for_tls_profile() {
    let profile = fixture_profile("dishwasher-tls.zip");
    let credentials = profile.credentials();

    assert_eq!(
        profile.identity().connection_type,
        ConnectionType::Tls,
        "the dishwasher is TLS"
    );
    assert_eq!(
        credentials.psk64.expose_secret().as_str(),
        "6Ev-lK2LOohvfBuOhWJAusL4vFG6jenXIKZ7zILScPS",
        "psk64 comes from the manifest's `key` on TLS too"
    );
    assert!(
        credentials.iv64.is_none(),
        "TLS has no app-layer crypto, so no iv64"
    );

    for fixture in FIXTURES.iter().filter(|f| f.appliance != "hob") {
        let profile = fixture_profile(fixture.file);
        assert!(
            profile.credentials().iv64.is_none(),
            "{} is TLS and must carry no iv64",
            fixture.appliance
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Host derivation, as a table over the whole fleet.
// ---------------------------------------------------------------------------------------------

#[test]
fn derives_host_for_all_four_real_profiles() {
    // One row per appliance. The AES row is `haId` alone; the three TLS rows are the composed
    // `{brand}-{type}-{haId}`. It reads as inverted only because an AES `haId` already is the
    // composed form.
    let table = [
        ("hob-aes.zip", "NEFF-T58TL6EN2-64DE7CA6DCB8"),
        ("dishwasher-tls.zip", "BOSCH-Dishwasher-237254714661314084"),
        ("oven-tls.zip", "BOSCH-Oven-111848791633551099"),
        ("fridge-tls.zip", "SIEMENS-FridgeFreezer-139555852472434992"),
    ];
    for (file, expected) in table {
        let profile = fixture_profile(file);
        assert_eq!(
            profile.derived_host(),
            expected,
            "derived host for the scrubbed {file}"
        );
        assert_eq!(
            profile.host(),
            expected,
            "with no manual override, host() is the derived host ({file})"
        );
    }

    // The same table against the real ZIPs, expressed as the rule plus the `haId` shapes, so no
    // real identifier is written down here.
    let visited = for_each_raw_profile(
        "derives_host_for_all_four_real_profiles",
        |fixture, profile| {
            let identity = profile.identity();
            let ha_id = identity.ha_id.expose_secret();
            if identity.connection_type.is_aes() {
                assert_eq!(
                    &profile.derived_host(),
                    ha_id,
                    "AES ({}) derives to haId alone",
                    fixture.appliance
                );
                assert!(
                    ha_id.starts_with(&format!("{}-{}-", identity.brand, identity.vib)),
                    "an AES haId is already the BRAND-VIB-MAC form ({})",
                    fixture.appliance
                );
            } else {
                assert_eq!(
                    profile.derived_host(),
                    format!("{}-{}-{ha_id}", identity.brand, identity.device_type),
                    "TLS ({}) derives to brand-type-haId",
                    fixture.appliance
                );
                assert_eq!(
                    Some(ha_id.as_str()),
                    identity
                        .serial_number
                        .as_ref()
                        .map(|serial| serial.expose_secret().as_str()),
                    "a TLS haId is the plain serial ({})",
                    fixture.appliance
                );
            }
        },
    );
    assert!(
        visited == 0 || visited == 4,
        "the gate is either shut or the whole fleet is present; saw {visited}"
    );
}

#[test]
fn manual_host_overrides_derivation() {
    // With static IPs, the manual override rather than derivation is the normal path.
    let profile = fixture_profile("hob-aes.zip").with_manual_host("192.0.2.55");

    assert_eq!(profile.host(), "192.0.2.55", "the override wins");
    assert_eq!(profile.manual_host(), Some("192.0.2.55"), "and is readable");
    assert_eq!(
        profile.derived_host(),
        "NEFF-T58TL6EN2-64DE7CA6DCB8",
        "derivation is still available underneath, unchanged"
    );

    let mut profile = profile;
    profile.set_manual_host(None);
    assert_eq!(
        profile.host(),
        "NEFF-T58TL6EN2-64DE7CA6DCB8",
        "clearing the override falls back to derivation"
    );
}

// ---------------------------------------------------------------------------------------------
// The entity model: types + access + min/max/step.
// ---------------------------------------------------------------------------------------------

#[test]
fn builds_entity_model_with_types_access_minmax_step() {
    let profile = fixture_profile("dishwasher-tls.zip");
    let model = profile.model();

    assert_eq!(
        model.meta().device_type.as_deref(),
        Some("Dishwasher"),
        "the <description> block is parsed"
    );

    assert_numeric_bounds(model);
    assert_enum_typed_status(model);
    assert_write_only_command(model);
    assert_containment(model);
    assert_program_option_overlay(model);
}

/// A `<setting>` with all three bounds: min / max / stepSize.
fn assert_numeric_bounds(model: &EntityModel) {
    let silence = model
        .entity_by_key("Dishcare.Dishwasher.Setting.SilenceOnDemandDefaultTime")
        .expect("the dishwasher declares SilenceOnDemandDefaultTime");
    assert_eq!(silence.uid, 0x111E, "uid is the wire's key");
    assert_eq!(silence.kind, EntityKind::Setting, "it is a <setting>");
    assert_eq!(
        silence.access.as_ref(),
        Some(&Access::ReadWrite),
        "access is readWrite"
    );
    assert_eq!(silence.available, Some(true), "available is true");
    let ValueType::Numeric(range) = silence.value_type else {
        panic!("expected a numeric range, got {:?}", silence.value_type)
    };
    assert_eq!(range.min, Some(60), "min");
    assert_eq!(range.max, Some(1800), "max");
    assert_eq!(range.step, Some(60), "stepSize");
}

/// A read-only `<status>` typed by an enumeration.
fn assert_enum_typed_status(model: &EntityModel) {
    let door = model
        .entity_by_key("BSH.Common.Status.DoorState")
        .expect("the dishwasher declares DoorState");
    assert_eq!(door.kind, EntityKind::Status, "it is a <status>");
    assert_eq!(door.access.as_ref(), Some(&Access::Read), "access is read");
    assert!(
        matches!(door.value_type, ValueType::Enumeration { enid } if enid == 0x0201),
        "DoorState is typed by enumeration 0201, got {:?}",
        door.value_type
    );
    assert!(
        door.access.as_ref().is_some_and(Access::is_readable),
        "and is readable"
    );
    assert!(
        !door.access.as_ref().is_some_and(Access::is_writable),
        "but not writable — the write path reads exactly this"
    );
}

/// A `<command>`: write-only, and no value type at all.
fn assert_write_only_command(model: &EntityModel) {
    let ack = model
        .entity_by_key("BSH.Common.Command.AcknowledgeEvent")
        .expect("the dishwasher declares AcknowledgeEvent");
    assert_eq!(ack.kind, EntityKind::Command, "it is a <command>");
    assert_eq!(
        ack.access.as_ref(),
        Some(&Access::WriteOnly),
        "a command is writeOnly"
    );
    assert_eq!(
        ack.value_type,
        ValueType::Unconstrained,
        "with neither an enumeration nor a range"
    );
}

/// Containers are entities too, and children point back at them.
fn assert_containment(model: &EntityModel) {
    let status_list = model
        .entity_by_key("BSH.Common.Root.StatusList")
        .expect("the root StatusList is itself a feature");
    assert_eq!(
        status_list.kind,
        EntityKind::StatusList,
        "the list is modelled"
    );
    let door = model
        .entity_by_key("BSH.Common.Status.DoorState")
        .expect("the dishwasher declares DoorState");
    assert_eq!(
        door.parent,
        Some(status_list.uid),
        "DoorState's parent is the StatusList that contains it"
    );
}

/// A `<program>` carries `<option refUID=…>` overlays rather than new uids.
fn assert_program_option_overlay(model: &EntityModel) {
    let eco = model
        .program_by_key("Dishcare.Dishwasher.Program.Eco50")
        .expect("Eco50 exists");
    let start_in_relative = eco
        .options
        .iter()
        .find(|option| option.ref_uid == 0x022E)
        .expect("Eco50 accepts BSH.Common.Option.StartInRelative — the dishwasher's start path");
    assert_eq!(
        start_in_relative.access,
        Some(Access::ReadWrite),
        "and it is writable within this program"
    );
    assert!(
        model.entity(0x022E).is_some(),
        "the overlay references a uid the description also declares in its own right"
    );
}

#[test]
fn resolves_enum_members_via_feature_mapping() {
    let profile = fixture_profile("fridge-tls.zip");
    let model = profile.model();

    let door_state = model
        .enum_type(0x0201)
        .expect("enumeration 0201 is declared by the description");
    assert_eq!(
        door_state.key.as_deref(),
        Some("BSH.Common.EnumType.DoorState"),
        "the enumKey comes from the FeatureMapping"
    );
    assert_eq!(door_state.member(0), Some("Open"), "0 = Open");
    assert_eq!(door_state.member(1), Some("Closed"), "1 = Closed");
    assert_eq!(door_state.member(2), Some("Locked"), "2 = Locked");
    assert_eq!(door_state.member(3), Some("Ajar"), "3 = Ajar");
    assert_eq!(
        door_state.declared_values,
        vec![0, 1, 2, 3],
        "the description declares which of them this appliance uses"
    );

    // Two enumerations that share member *names* but not values are the reason resolution has
    // to go through the enid rather than through the name.
    let fridge_door = model
        .enum_type(0x1000)
        .expect("Refrigeration.Common.EnumType.Door.States");
    assert_eq!(
        fridge_door.key.as_deref(),
        Some("Refrigeration.Common.EnumType.Door.States"),
        "a second, appliance-specific door enumeration"
    );
    assert_eq!(fridge_door.member(0), Some("Closed"), "0 = Closed here");
    assert_eq!(fridge_door.member(1), Some("Open"), "1 = Open here");
    assert_ne!(
        fridge_door.member(0),
        door_state.member(0),
        "the same value means different things in different enumerations"
    );

    // And an entity resolves to its own enumeration.
    let refrigerator_door = model
        .entity_by_key("Refrigeration.Common.Status.Door.Refrigerator")
        .expect("the fridge declares its refrigerator door status");
    let resolved = model
        .enum_type_of(refrigerator_door)
        .expect("the entity resolves to an enumeration");
    assert_eq!(resolved.enid, 0x1000, "via enumerationType, not by name");
}

// ---------------------------------------------------------------------------------------------
// One malformed entity is skipped, never fatal.
// ---------------------------------------------------------------------------------------------

// Two separate tests, so a leaf-case failure cannot mask the container case.

/// The leaf case: a `<status>` whose uid cannot be read.
#[test]
fn one_malformed_entity_is_skipped_not_fatal_leaf_case() {
    let baseline = fixture_profile("hob-aes.zip");
    let baseline_count = baseline.model().entities().len();
    assert!(
        baseline_count > 100,
        "the hob is the busy description; got {baseline_count} entities"
    );
    assert!(
        baseline.skipped().is_empty(),
        "the unmodified hob profile skips nothing"
    );

    // Break exactly one entity: BSH.Common.Status.BackendConnected (uid 0005) gets a uid that is
    // not a 16-bit hex number. Everything around it is untouched.
    let victim_uid = 0x0005u16;
    let mut members = read_zip(&fixture_bytes("hob-aes.zip"));
    let mut broke = 0usize;
    for (name, body) in &mut members {
        if !name.ends_with("_DeviceDescription.xml") {
            continue;
        }
        let text = String::from_utf8(body.clone()).unwrap();
        let broken = text.replacen(
            r#"<status access="read" available="true" refCID="01" refDID="00" uid="0005"/>"#,
            r#"<status access="read" available="true" refCID="01" refDID="00" uid="NOT-HEX"/>"#,
            1,
        );
        assert_ne!(broken, text, "the injection site must exist in {name}");
        broke += 1;
        *body = broken.into_bytes();
    }
    assert_eq!(broke, 1, "exactly one description was modified");

    let damaged = Profile::from_zip_bytes(&build_zip(&members))
        .expect("one malformed entity must not fail the whole profile");

    assert_eq!(
        damaged.model().entities().len(),
        baseline_count - 1,
        "every other entity still loaded"
    );
    assert!(
        damaged.model().entity(victim_uid).is_none(),
        "the broken entity is absent"
    );
    assert_eq!(damaged.skipped().len(), 1, "and is reported, exactly once");
    let skipped = damaged.skipped().first().expect("exactly one skip");
    assert_eq!(skipped.element, "status", "the element is named");
    assert_eq!(
        skipped.raw_uid.as_deref(),
        Some("NOT-HEX"),
        "with the uid as written"
    );
    assert_eq!(
        skipped.reason,
        SkipReason::UnparsableUid,
        "and why it was dropped"
    );

    // The device is still usable: identity, credentials and programs all survived.
    assert_eq!(
        damaged.derived_host(),
        baseline.derived_host(),
        "the device is still addressable"
    );
    assert_eq!(
        damaged.model().programs().len(),
        baseline.model().programs().len(),
        "the hob's programs are untouched"
    );
}

/// The container case.
///
/// If a skipped container's open is not pushed but its close still pops, the grandparent comes
/// off the stack and every later entity silently gets the wrong `parent`.
///
/// The fridge nests `Refrigeration.Common.SettingList.Light.Internal` (uid `5000`) inside
/// `BSH.Common.Root.SettingList` (uid `0103`), with a sibling list (`5010`) and a plain setting
/// (`2024`) after it — so breaking `5000` is exactly the shape that exposes the imbalance.
#[test]
fn one_malformed_entity_is_skipped_not_fatal_container_case() {
    let baseline = fixture_profile("fridge-tls.zip");
    assert!(
        baseline.skipped().is_empty(),
        "the unmodified fridge profile skips nothing"
    );
    assert_eq!(
        baseline
            .model()
            .entity(BROKEN_CONTAINER)
            .map(|entity| entity.parent),
        Some(Some(CONTAINER_GRANDPARENT)),
        "the fixture really does nest one settingList inside another"
    );

    let damaged = Profile::from_zip_bytes(&zip_with_a_broken_container())
        .expect("one malformed container must not fail the whole profile");

    assert_eq!(
        damaged.model().entities().len(),
        baseline.model().entities().len() - 1,
        "the container's children are still entities; only the container itself is gone"
    );
    assert!(
        damaged.model().entity(BROKEN_CONTAINER).is_none(),
        "the broken container is absent"
    );
    assert_container_skip_record(&damaged);
    assert_parent_linkage_survives(&baseline, &damaged);
}

/// The container whose uid the test breaks:
/// `Refrigeration.Common.SettingList.Light.Internal`.
const BROKEN_CONTAINER: u16 = 0x5000;
/// The list that encloses it, which an unbalanced pop would remove from the stack.
const CONTAINER_GRANDPARENT: u16 = 0x0103;
/// A sibling container declared *after* the victim, inside the same grandparent.
const LATER_SIBLING_LIST: u16 = 0x5010;
/// A child of that sibling list, so the stack is checked below the damage too.
const LATER_SIBLING_CHILD: u16 = 0x5011;
/// A plain setting declared after the victim, inside the same grandparent.
const LATER_SIBLING_LEAF: u16 = 0x2024;

/// The fridge fixture with [`BROKEN_CONTAINER`]'s `uid` replaced by a non-hex string.
fn zip_with_a_broken_container() -> Vec<u8> {
    let mut members = read_zip(&fixture_bytes("fridge-tls.zip"));
    let mut broke = 0usize;
    for (name, body) in &mut members {
        if !name.ends_with("_DeviceDescription.xml") {
            continue;
        }
        let text = String::from_utf8(body.clone()).unwrap();
        let broken = text.replacen(
            r#"<settingList access="readWrite" available="true" uid="5000">"#,
            r#"<settingList access="readWrite" available="true" uid="NOT-HEX">"#,
            1,
        );
        assert_ne!(broken, text, "the injection site must exist in {name}");
        broke += 1;
        *body = broken.into_bytes();
    }
    assert_eq!(broke, 1, "exactly one description was modified");
    build_zip(&members)
}

/// One bad container is one skip record: not a cascade, and not silence.
fn assert_container_skip_record(damaged: &Profile) {
    assert_eq!(
        damaged.skipped().len(),
        1,
        "one bad container is one skip record"
    );
    let skipped = damaged.skipped().first().expect("exactly one skip");
    assert_eq!(skipped.element, "settingList", "the element is named");
    assert_eq!(
        skipped.raw_uid.as_deref(),
        Some("NOT-HEX"),
        "with the uid as written"
    );
    assert_eq!(
        skipped.reason,
        SkipReason::UnparsableUid,
        "and why it was dropped"
    );
}

/// The blast radius is the victim and its own children, nothing else.
fn assert_parent_linkage_survives(baseline: &Profile, damaged: &Profile) {
    for (uid, before) in baseline.model().entities() {
        if *uid == BROKEN_CONTAINER {
            continue;
        }
        let after = damaged
            .model()
            .entity(*uid)
            .unwrap_or_else(|| panic!("entity {uid:04X} still exists"));
        if before.parent == Some(BROKEN_CONTAINER) {
            assert_eq!(
                after.parent, None,
                "a child of the skipped container has no addressable parent left ({uid:04X})"
            );
        } else {
            assert_eq!(
                after.parent, before.parent,
                "parent linkage is unchanged for {uid:04X}"
            );
        }
    }

    // Spelled out for the three an unbalanced stack would move, so a regression names itself.
    for (uid, expected) in [
        (LATER_SIBLING_LIST, CONTAINER_GRANDPARENT),
        (LATER_SIBLING_LEAF, CONTAINER_GRANDPARENT),
        (LATER_SIBLING_CHILD, LATER_SIBLING_LIST),
    ] {
        assert_eq!(
            damaged.model().entity(uid).and_then(|entity| entity.parent),
            Some(expected),
            "{uid:04X} is still parented by {expected:04X} after the skipped container"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// A `<description/>` written as an empty element must not latch the walker shut.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_self_closing_description_does_not_swallow_the_document() {
    let baseline = fixture_profile("fridge-tls.zip");
    let baseline_count = baseline.model().entities().len();
    assert!(baseline_count > 0, "the baseline has entities to lose");
    assert_eq!(
        baseline.model().meta().device_type.as_deref(),
        Some("FridgeFreezer"),
        "and a <description> block that really is parsed"
    );

    // `<description>…</description>` becomes `<description/>`: a truncated or hostile export.
    // `Event::Empty` never produces a matching `End`, so a walker that sets an `in_description`
    // flag on open and clears it on close stays latched for the rest of the document.
    let mut members = read_zip(&fixture_bytes("fridge-tls.zip"));
    let mut rewrote = 0usize;
    for (name, body) in &mut members {
        if !name.ends_with("_DeviceDescription.xml") {
            continue;
        }
        let text = String::from_utf8(body.clone()).unwrap();
        let (head, rest) = text
            .split_once("<description>")
            .expect("the fixture opens a <description> block");
        let (_, tail) = rest
            .split_once("</description>")
            .expect("and closes it again");
        *body = format!("{head}<description/>{tail}").into_bytes();
        rewrote += 1;
    }
    assert_eq!(rewrote, 1, "exactly one description was rewritten");

    let emptied = Profile::from_zip_bytes(&build_zip(&members))
        .expect("an empty <description/> is well-formed XML");

    assert_eq!(
        emptied.model().entities().len(),
        baseline_count,
        "every entity after the empty <description/> must still be read"
    );
    assert!(
        emptied.model().meta().device_type.is_none(),
        "the metadata really is gone — the element carries nothing now"
    );
    assert!(
        emptied.skipped().is_empty(),
        "and nothing was skipped getting there"
    );
}

// ---------------------------------------------------------------------------------------------
// `haId` is a secret too: on AES it contains the MAC.
// ---------------------------------------------------------------------------------------------

#[test]
fn ha_id_does_not_survive_a_debug_format() {
    let profile = fixture_profile("hob-aes.zip");
    let identity = profile.identity();

    let rendered = format!("{identity:?}");
    assert!(
        rendered.contains("redacted"),
        "DeviceIdentity must render redacted, got {rendered}"
    );
    // On the AES path the haId carries the MAC, which the `mac` field already wraps, so leaving
    // haId bare would print it anyway.
    assert!(
        !rendered.contains("64DE7CA6DCB8"),
        "the haId must not survive a Debug format: {rendered}"
    );
    assert!(
        !rendered.contains("64-DE-7C-A6-DC-B8"),
        "and neither must the mac it is built from: {rendered}"
    );
    assert!(
        !rendered.contains("682041458357108136"),
        "nor the serial number: {rendered}"
    );
    assert!(
        !rendered.contains(identity.ha_id.expose_secret().as_str()),
        "and the check above really is against the value this profile carries"
    );

    // Wrapping the field does not push `Secret` into the connect string: the derived host stays
    // a plain `String`.
    assert_eq!(
        profile.host(),
        "NEFF-T58TL6EN2-64DE7CA6DCB8",
        "the value still reaches the transport in the clear, which is the point of a Secret"
    );
    assert_eq!(
        profile.derived_host(),
        "NEFF-T58TL6EN2-64DE7CA6DCB8",
        "derivation is unchanged"
    );
}

// ---------------------------------------------------------------------------------------------
// The ZIP is untrusted input.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_zip_member_that_inflates_past_the_cap_is_rejected() {
    let cap = usize::try_from(hc_profile::MAX_MEMBER_BYTES).expect("the cap fits a usize");
    let bomb = vec![b' '; cap + 1];

    // The manifest is located by extension and inflated before anything is known about it, so
    // that is the member an attacker gets to choose the size of first.
    let archive = build_deflated_zip(&[("manifest.json".to_owned(), bomb.clone())]);
    assert!(
        archive.len() < 128 * 1024,
        "the archive itself is tiny — that is the whole attack; got {} B",
        archive.len()
    );
    match Profile::from_zip_bytes(&archive) {
        Err(ProfileError::MemberTooLarge { member, limit }) => {
            assert_eq!(member, "manifest.json", "the offending member is named");
            assert_eq!(limit, hc_profile::MAX_MEMBER_BYTES, "and the cap is quoted");
        }
        other => panic!("expected MemberTooLarge for the manifest, got {other:?}"),
    }

    // …and the same for a member the manifest declares. Here the manifest is honest and small;
    // it is the description that is the bomb.
    let mut members = read_zip(&fixture_bytes("fridge-tls.zip"));
    for (name, body) in &mut members {
        if name.ends_with("_DeviceDescription.xml") {
            *body = bomb.clone();
        }
    }
    match Profile::from_zip_bytes(&build_deflated_zip(&members)) {
        Err(ProfileError::MemberTooLarge { member, .. }) => {
            assert!(
                member.ends_with("_DeviceDescription.xml"),
                "the declared member is named, got {member}"
            );
        }
        other => panic!("expected MemberTooLarge for the description, got {other:?}"),
    }

    // The cap is a ceiling, not a budget: all four real profiles parse well under it.
    for fixture in &FIXTURES {
        let _ = fixture_profile(fixture.file);
    }
}

// ---------------------------------------------------------------------------------------------
// A malformed 426 KB document must say *where*, not just *which file*.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_malformed_xml_member_reports_the_position_of_the_fault() {
    let mut members = read_zip(&fixture_bytes("fridge-tls.zip"));
    let mut fault_at = None;
    for (name, body) in &mut members {
        if !name.ends_with("_DeviceDescription.xml") {
            continue;
        }
        let text = String::from_utf8(body.clone()).unwrap();
        // Truncate inside an element start tag, well into the document, like a half-downloaded
        // export.
        let (head, _) = text
            .split_once("<settingList")
            .expect("the fixture has a settingList");
        fault_at = Some(head.len() as u64);
        *body = format!("{head}<settingList access=\"readWrite\"").into_bytes();
    }
    let fault_at = fault_at.expect("exactly one description was truncated");
    assert!(
        fault_at > 1024,
        "the fault is deep enough that a bare filename would be useless; at {fault_at}"
    );

    match Profile::from_zip_bytes(&build_zip(&members)) {
        Err(ProfileError::Xml {
            member, position, ..
        }) => {
            assert!(
                member.ends_with("_DeviceDescription.xml"),
                "the member is named, got {member}"
            );
            assert_eq!(
                position, fault_at,
                "and the position points at the `<` that never closed"
            );
        }
        other => panic!("expected a positioned Xml error, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// The 426 KB stress case.
// ---------------------------------------------------------------------------------------------

/// Wall-clock ceiling for one full parse of the oven profile.
///
/// Not a performance target but a tripwire for an accidentally quadratic parser. A debug build
/// parses this in well under a second, so 3 s leaves room for a loaded CI runner.
const OVEN_PARSE_BUDGET: Duration = Duration::from_secs(3);

#[test]
fn oven_description_parses_under_budget() {
    let bytes = fixture_bytes("oven-tls.zip");

    let uncompressed: usize = read_zip(&bytes)
        .iter()
        .filter(|(name, _)| name.ends_with("_DeviceDescription.xml"))
        .map(|(_, body)| body.len())
        .sum();
    assert!(
        uncompressed > 400 * 1024,
        "this test is only a stress case while the description is ~426 KB; got {uncompressed} B"
    );

    let started = Instant::now();
    let profile = Profile::from_zip_bytes(&bytes).expect("the oven profile parses");
    let elapsed = started.elapsed();

    assert!(
        elapsed < OVEN_PARSE_BUDGET,
        "parsing the oven profile took {elapsed:?}, over the {OVEN_PARSE_BUDGET:?} budget"
    );
    assert!(
        profile.model().entities().len() > 500,
        "and it really did parse the whole thing: {} entities",
        profile.model().entities().len()
    );
    assert!(
        profile.model().programs().len() > 100,
        "including {} programs",
        profile.model().programs().len()
    );
}

// ---------------------------------------------------------------------------------------------
// The degenerate case: an appliance with no program surface at all.
// ---------------------------------------------------------------------------------------------

#[test]
fn fridge_with_no_programs_yields_empty_program_set() {
    let profile = fixture_profile("fridge-tls.zip");
    let model = profile.model();

    assert!(
        model.programs().is_empty(),
        "the fridge has no programs; that is an empty set, not an error"
    );

    // The slots still exist (an empty <programGroup/>, plus the active/selected program
    // pointers) and must be modelled, or features would be lost.
    let group = model
        .entity_by_key("BSH.Common.Root.ProgramGroup")
        .expect("the empty programGroup is still a feature");
    assert_eq!(
        group.kind,
        EntityKind::ProgramGroup,
        "an empty group is still a group"
    );
    assert!(
        model
            .entity_by_key("BSH.Common.Root.SelectedProgram")
            .is_some(),
        "SelectedProgram exists even with nothing to select"
    );
    assert!(
        model
            .entity_by_key("BSH.Common.Root.ActiveProgram")
            .is_some(),
        "and so does ActiveProgram"
    );

    // The rest of the model is entirely normal.
    assert!(
        !model.entities().is_empty(),
        "the fridge still exposes statuses, settings, events and commands"
    );
    assert!(
        profile.skipped().is_empty(),
        "and nothing was skipped getting there"
    );

    // The other three appliances do have programs, so the assertion above is about the fridge,
    // not the parser.
    for fixture in FIXTURES.iter().filter(|f| f.appliance != "fridge") {
        let other = fixture_profile(fixture.file);
        assert!(
            !other.model().programs().is_empty(),
            "{} has programs, so an empty set really is fridge-specific",
            fixture.appliance
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The raw tier, kept honest: whatever the scrubbed fixtures prove must hold on the real files.
// ---------------------------------------------------------------------------------------------

#[test]
fn real_profiles_parse_with_the_same_shape_as_the_scrubbed_fixtures() {
    let visited = for_each_raw_profile(
        "real_profiles_parse_with_the_same_shape_as_the_scrubbed_fixtures",
        |fixture: &Fixture, real| {
            let scrubbed = fixture_profile(fixture.file);
            assert_eq!(
                real.model().entities().len(),
                scrubbed.model().entities().len(),
                "scrubbing must not change the entity count for the {}",
                fixture.appliance
            );
            assert_eq!(
                real.model().programs().len(),
                scrubbed.model().programs().len(),
                "nor the program count for the {}",
                fixture.appliance
            );
            assert_eq!(
                real.model().enum_types().len(),
                scrubbed.model().enum_types().len(),
                "nor the enumeration count for the {}",
                fixture.appliance
            );
            assert!(
                real.skipped().is_empty(),
                "no real profile has an unparsable entity ({})",
                fixture.appliance
            );
            assert_eq!(
                real.identity().connection_type,
                scrubbed.identity().connection_type,
                "nor the connection type for the {}",
                fixture.appliance
            );
        },
    );
    assert!(
        visited == 0 || visited == 4,
        "the gate is either shut or the whole fleet is present; saw {visited}"
    );
}

/// The profile XML spells access words in camelCase; the appliance's `/ro/descriptionChange`
/// items spell them in upper case (`"READ"`, `"NONE"`). Both mean the same thing.
#[test]
fn access_words_parse_in_either_spelling() {
    use hc_profile::Access;
    for (xml, wire, access) in [
        ("none", "NONE", Access::None),
        ("read", "READ", Access::Read),
        ("readStatic", "READSTATIC", Access::ReadStatic),
        ("writeOnly", "WRITEONLY", Access::WriteOnly),
        ("readWrite", "READWRITE", Access::ReadWrite),
    ] {
        assert_eq!(Access::parse(xml), access, "{xml}");
        assert_eq!(Access::parse(wire), access, "{wire}");
    }
}
