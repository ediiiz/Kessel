//! The display-label layer: what a feature key, an enum member and a program are *called*.
//!
//! The appliance archive supplies no human-readable names (see `server::labels`), so these tests pin
//! the two things that are ours: the derived spelling fallback, and the layered lookup that lets a
//! contributed per-model entry override it.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use server::labels::{LabelSource, LabelTable, embedded, humanize, key_prefix};

/// Every contributed file parses, and every one sits somewhere the loader recognises.
///
/// Both tables carry `deny_unknown_fields`, so a mistyped key fails here rather than being
/// ignored at run time. This is also what lets `embedded()` promise it cannot panic.
#[test]
fn contributed_files_all_parse() {
    let table = LabelTable::from_embedded();
    assert!(table.is_ok(), "a contributed file is malformed: {table:?}");
}

/// Every contributed key is one an appliance actually declares.
///
/// A missing key shows up as a derived name; a wrong key (a typo, a cloud-API spelling) names
/// nothing and silently looks like coverage. So a contributed key must name something one of the
/// four real descriptions declares, or be listed below as a deliberate exception for a feature seen
/// on an appliance there is no fixture for.
const KEYS_NO_FIXTURE_DECLARES: [&str; 6] = [
    // Real BSH features; none of our four appliances has an ambient light.
    "BSH.Common.Setting.AmbientLightBrightness",
    "BSH.Common.Setting.AmbientLightEnabled",
    // Real dishwasher features. The SMD8TCX01E declares neither the half-load option nor these
    // three programmes; other Bosch/Siemens dishwashers do.
    "Dishcare.Dishwasher.Option.HalfLoad",
    "Dishcare.Dishwasher.Program.Auto1",
    "Dishcare.Dishwasher.Program.Auto3",
    "Dishcare.Dishwasher.Program.Quick45",
];

#[test]
fn every_contributed_key_names_a_feature_some_appliance_declares() {
    let declared = declared_keys();
    let contributed = server::labels::contributed_keys();
    assert!(
        contributed.len() > 100,
        "only {} contributed keys — this gate is only worth having over a real catalogue",
        contributed.len()
    );

    let unmatched: Vec<&String> = contributed
        .iter()
        .filter(|key| !declared.contains(*key))
        .filter(|key| !KEYS_NO_FIXTURE_DECLARES.contains(&key.as_str()))
        .collect();
    assert!(
        unmatched.is_empty(),
        "{} contributed key(s) name nothing any of the four appliances declares and are not listed \
         in KEYS_NO_FIXTURE_DECLARES — a typo or a cloud-API spelling looks like coverage and is \
         silent: {unmatched:#?}",
        unmatched.len()
    );

    // The exception list is itself checked: an entry that a fixture *does* declare has stopped being
    // an exception, and leaving it here would hide the next real one.
    let stale: Vec<&&str> = KEYS_NO_FIXTURE_DECLARES
        .iter()
        .filter(|key| declared.contains(**key))
        .collect();
    assert!(
        stale.is_empty(),
        "these keys are now declared by a fixture and must come off the exception list: {stale:#?}"
    );
}

/// Every dotted key the four real descriptions name, features and programmes together.
fn declared_keys() -> std::collections::BTreeSet<String> {
    let mut keys = std::collections::BTreeSet::new();
    for fixture in [
        "hob-aes.zip",
        "fridge-tls.zip",
        "dishwasher-tls.zip",
        "oven-tls.zip",
    ] {
        let bytes = std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/hc-profile/tests/fixtures")
                .join(fixture),
        )
        .unwrap_or_else(|err| panic!("reading {fixture}: {err}"));
        let profile = hc_profile::Profile::from_zip_bytes(&bytes)
            .unwrap_or_else(|err| panic!("parsing {fixture}: {err}"));
        keys.extend(
            profile
                .model()
                .entities()
                .values()
                .filter_map(|entity| entity.key.clone()),
        );
    }
    keys
}

/// The model layer wins over the prefix layer, which wins over the derived spelling.
#[test]
fn the_most_specific_layer_wins() {
    let bosch = embedded().for_device("BOSCH", "SMD8TCX01E");

    // Model layer: this exact appliance names TimeLight, and the prefix files do not.
    let time_light = bosch.feature("Dishcare.Dishwasher.Setting.TimeLight");
    assert_eq!(time_light.text, "Time beam");
    assert_eq!(time_light.source, LabelSource::Model);
    assert!(time_light.description.is_some());

    // Prefix layer: every dishwasher shares this one, and no model file overrides it.
    let eco = bosch.feature("Dishcare.Dishwasher.Setting.EcoAsDefault");
    assert_eq!(eco.text, "Default programme");
    assert_eq!(eco.source, LabelSource::Prefix);

    // Derived: nobody named it, so it is the identifier re-spelled — never blank, never "Unknown".
    let unmapped = bosch.feature("Dishcare.Dishwasher.Setting.NoOneNamedThis");
    assert_eq!(unmapped.text, "No one named this");
    assert_eq!(unmapped.source, LabelSource::Derived);
    assert_eq!(unmapped.description, None);
}

/// A model with no contributed file still gets the prefix and derived layers.
///
/// This is the common case, so it must not regress to blank labels.
#[test]
fn an_unknown_model_falls_through_to_the_shared_layers() {
    let unknown = embedded().for_device("NEFF", "T58TL6EN2");

    assert_eq!(
        unknown
            .feature("Dishcare.Dishwasher.Setting.EcoAsDefault")
            .source,
        LabelSource::Prefix
    );

    // TimeLight is named only by the BOSCH model file, so this model derives it.
    let time_light = unknown.feature("Dishcare.Dishwasher.Setting.TimeLight");
    assert_eq!(time_light.text, "Time light");
    assert_eq!(time_light.source, LabelSource::Derived);
}

/// Enum members resolve per member, and an unnamed member derives its own spelling.
///
/// Without this, raw member names such as `LastProgram` / `EcoAsDefault` become button labels.
#[test]
fn enum_members_resolve_independently_of_their_feature() {
    let bosch = embedded().for_device("BOSCH", "SMD8TCX01E");
    let key = "Dishcare.Dishwasher.Setting.EcoAsDefault";

    let last = bosch.member(key, "LastProgram");
    assert_eq!(last.text, "Whatever ran last");
    assert_eq!(last.source, LabelSource::Prefix);

    // An unnamed member derives from *its own* name, not from the feature's label.
    let invented = bosch.member(key, "SomeNewMember");
    assert_eq!(invented.text, "Some new member");
    assert_eq!(invented.source, LabelSource::Derived);
}

/// Programs resolve from their own table, not from `[features]`.
#[test]
fn programs_resolve_from_the_programs_table() {
    let bosch = embedded().for_device("BOSCH", "SMD8TCX01E");

    let eco = bosch.program("Dishcare.Dishwasher.Program.Eco50");
    assert_eq!(eco.text, "Eco 50 °C");
    assert_eq!(eco.source, LabelSource::Prefix);

    // A program nobody named still reads as a programme name rather than as a dotted key.
    let unnamed = bosch.program("Dishcare.Dishwasher.Program.Quick65");
    assert_eq!(unnamed.text, "Quick 65");
    assert_eq!(unnamed.source, LabelSource::Derived);
}

/// The prefix is the key's **first two** segments.
///
/// `apps/web`'s `groupForFeatureKey` computes "everything before the last two", which for the
/// six-segment favourite keys gives `BSH.Common.Setting.Favorite` — that would miss the `BSH.Common`
/// file entirely. The two functions are deliberately not the same computation.
#[test]
fn the_prefix_is_the_first_two_segments() {
    assert_eq!(
        key_prefix("BSH.Common.Setting.Favorite.001.Functionality"),
        "BSH.Common"
    );
    assert_eq!(
        key_prefix("Dishcare.Dishwasher.Setting.TimeLight"),
        "Dishcare.Dishwasher"
    );
    // Fewer than three segments: the key is its own prefix.
    assert_eq!(key_prefix("BSH.Common"), "BSH.Common");
    assert_eq!(key_prefix("Solo"), "Solo");
}

/// The derived fallback spells a camel-case identifier as a sentence without inventing a word.
///
/// Every case here is a real segment from the four appliances' feature keys or enum members.
#[test]
fn derived_labels_are_sentence_case() {
    // The ordinary camel hump.
    assert_eq!(humanize("TimeLight"), "Time light");
    assert_eq!(humanize("TemperatureUnit"), "Temperature unit");
    assert_eq!(humanize("RemainingProgramTime"), "Remaining programme time");

    // Real enum members.
    assert_eq!(humanize("EcoAsDefault"), "Eco as default");
    assert_eq!(humanize("LastProgram"), "Last programme");
    assert_eq!(humanize("ApplianceOnDoorOpen"), "Appliance on door open");
    assert_eq!(humanize("AlwaysOnDoorOpen"), "Always on door open");

    // A single word is already a sentence: no separator to insert, no case to change.
    assert_eq!(humanize("Off"), "Off");
    assert_eq!(humanize("On"), "On");
    assert_eq!(humanize("Functionality"), "Functionality");
}

/// The appliance's American `Program` is spelled the way the catalogue spells it, wherever it sits.
#[test]
fn a_derived_label_spells_programme_like_the_catalogue() {
    assert_eq!(humanize("Program"), "Programme");
    assert_eq!(humanize("ProgramAborted"), "Programme aborted");
    assert_eq!(humanize("AllPrograms"), "All programmes");
    // A word that merely starts with it is left alone.
    assert_eq!(humanize("ProgramProgress"), "Programme progress");
    assert_eq!(humanize("Programmable"), "Programmable");
}

/// Digits break a token in both directions, and an acronym run is not re-cased.
///
/// `Eco50` is a real dishwasher program suffix and `LED` a real hob one. Lower-casing an acronym
/// (`Led`) would be the transform inventing a word that was never in the identifier, which rule 1
/// of `server::labels` forbids.
#[test]
fn derived_labels_keep_acronyms_and_split_digits() {
    assert_eq!(humanize("Eco50"), "Eco 50");
    assert_eq!(humanize("Intensiv70"), "Intensiv 70");
    assert_eq!(humanize("LEDBrightness"), "LED brightness");
    assert_eq!(humanize("BSHKey"), "BSH key");
    assert_eq!(humanize("LED"), "LED");
}

/// A proper noun that spans two camel humps keeps both capitals.
///
/// The sentence-case pass would turn `WiFi` into `Wi fi`, and no character-local rule tells
/// `WiFi` from `TimeLight`, so there is a small table of proper nouns.
///
/// `Wi Fi`, not `WiFi`: that is how Home Assistant's own `home_connect` integration spells it.
#[test]
fn a_known_compound_keeps_its_capitals() {
    assert_eq!(humanize("WiFi"), "Wi Fi");
    assert_eq!(humanize("DeactivateWiFi"), "Deactivate Wi Fi");
    assert_eq!(humanize("WiFiSignalStrength"), "Wi Fi signal strength");
    assert_eq!(
        humanize("AllowSoftwareUpdateLocalWiFi"),
        "Allow software update local Wi Fi"
    );

    // The table is for proper nouns only. An ordinary word that happens to follow another is still
    // lower-cased — `Cooking.Oven.Program.Meat.BonelessLegOfPork` and its seven siblings would read
    // very oddly otherwise.
    assert_eq!(humanize("BonelessLegOfPork"), "Boneless leg of pork");
    assert_eq!(humanize("SwitchOffAfter"), "Switch off after");
}

/// A number is not a name: a key whose last segment is all digits takes the segment before it.
///
/// Otherwise `BSH.Common.Program.Favorite.001` derives to `001`. Across the four fixtures three
/// prefixes take a numeric segment: `Favorite`, the hob's `Zone` and the oven's `Cavity`. Both
/// words in the output are already in the identifier, and a contributed name can still override it.
#[test]
fn a_numeric_last_segment_is_an_index_not_a_name() {
    // No model layer and no contributed entry for these, so every one is the derived answer.
    let labels = embedded().for_device("", "");

    assert_eq!(
        labels.program("BSH.Common.Program.Favorite.001").text,
        "Favorite 001",
        "the option the oven offers thirty of"
    );
    assert_eq!(
        labels.feature("Cooking.Hob.Setting.Zone.001").text,
        "Zone 001"
    );

    // One segment back is enough, and it is all that is taken: a non-numeric leaf is untouched, and
    // the digits inside an ordinary leaf are not an index.
    assert_eq!(
        labels
            .feature("BSH.Common.Setting.Favorite.001.Program")
            .text,
        "Programme"
    );
    assert_eq!(
        labels
            .program("Dishcare.Dishwasher.Program.NoOneNamedThis50")
            .text,
        "No one named this 50"
    );
}

/// Degenerate input is returned, not rejected.
///
/// `humanize` is the fallback, so it may never fail to produce something: an empty or
/// already-spaced segment keeps whatever it had.
#[test]
fn derived_labels_have_an_identity_fallback() {
    assert_eq!(humanize(""), "");
    assert_eq!(humanize("Time light"), "Time light");
    assert_eq!(humanize("_"), "_");
}

/// The catalogue that names features in the web UI is the same one that names them in Home
/// Assistant.
///
/// `hc_core::discovery` owns no name table: its `FeatureNaming` trait asks, and this crate answers.
/// A name that differs between the web UI and the HA device page leaves the operator unable to tell
/// which is stale. `hc-core` cannot see this table, so the property is asserted here, for the
/// derived layer as well as the contributed one.
#[test]
fn home_assistant_is_named_from_the_same_catalogue_as_the_web_ui() {
    let names = server::mqtt::CatalogueNames::for_device("BOSCH", "SMD8TCX01E");
    let labels = embedded().for_device("BOSCH", "SMD8TCX01E");

    for key in [
        // Model layer.
        "Dishcare.Dishwasher.Setting.TimeLight",
        // Prefix layer.
        "Dishcare.Dishwasher.Setting.EcoAsDefault",
        // Derived layer — the spelling transform, which `hc-core` would otherwise not have.
        "Dishcare.Dishwasher.Setting.NoOneNamedThis",
    ] {
        assert_eq!(
            hc_core::discovery::FeatureNaming::feature_name(&names, key),
            Some(labels.feature(key).text),
            "`{key}`: HA and the web UI must be given the same name from the same table"
        );
    }
}

/// The two surfaces render the same feature the same way.
///
/// The test above covers the feature's name. This one covers member names and descriptions, which
/// `state.rs::entity_descriptors` and the MQTT discovery path must resolve identically.
#[test]
fn home_assistant_and_the_web_ui_render_the_same_feature_the_same_way() {
    let key = "BSH.Common.Setting.TemperatureUnit";
    let labels = embedded().for_device("BOSCH", "SMD8TCX01E");
    let catalogue = server::mqtt::CatalogueNames::for_device("BOSCH", "SMD8TCX01E");

    // What the web UI shows: a contributed name per member, keyed by the raw member.
    let members = ["Celsius", "Fahrenheit"];
    let web = labels.member_labels(key, members);
    assert_eq!(
        web.get("Celsius").map(String::as_str),
        Some("°C"),
        "premise: this feature has contributed member names"
    );

    for member in members {
        assert_eq!(
            hc_core::discovery::FeatureNaming::member_name(&catalogue, key, member),
            web.get(member).cloned(),
            "`{key}` / `{member}`: HA and the web UI must render the same member the same way"
        );
    }

    // …and it reaches the payload, not just the trait. The dishwasher is the appliance with
    // contributed member names on a feature it actually declares.
    let dishwasher = server::mqtt::CatalogueNames::for_device("BOSCH", "SMD8TCX01E");
    let bytes = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/hc-profile/tests/fixtures/dishwasher-tls.zip"),
    )
    .expect("the dishwasher fixture is readable");
    let profile = hc_profile::Profile::from_zip_bytes(&bytes).expect("parses");
    let device = hc_core::mapping::DeviceId::new("dishwasher");
    let mapping = hc_core::mapping::DeviceMapping::build(
        &device,
        profile.model(),
        &hc_core::mapping::Excludes::none(),
    );
    let info = hc_core::discovery::DeviceInfo::default();
    let configs = hc_core::discovery::discovery_configs(
        "kessel",
        &device,
        &mapping,
        &hc_core::discovery::Announcement {
            mode: hc_core::discovery::DiscoveryMode::Full,
            info: &info,
            names: &dishwasher,
            origin: &hc_core::discovery::DiscoveryOrigin::KESSEL,
        },
    );
    let eco = config_for_slot(&configs, "Dishcare_Dishwasher_Setting_EcoAsDefault");
    assert_eq!(
        eco.get("options"),
        Some(&serde_json::json!(["Whatever ran last", "Always Eco"])),
        "the contributed member names reach Home Assistant's `options`, in the appliance's own \
         declared order"
    );

    assert_eq!(
        hc_core::discovery::FeatureNaming::feature_description(
            &catalogue,
            "BSH.Common.Status.RemoteControlStartAllowed"
        ),
        labels
            .feature("BSH.Common.Status.RemoteControlStartAllowed")
            .description,
        "a contributed description is help text on both surfaces or neither"
    );
}

/// A programme chooser's options are programmes, which have their own namespace.
///
/// `mapping::program_options` fills `<selectedProgram>`'s `options` with programme keys, which are
/// named from `labels`'s `[programs]` table, not from a feature's `values`. The web UI uses the same
/// table (`state.rs::program_descriptors`). The expectation is computed through `labels::program`
/// so it stays right when the catalogue grows.
#[test]
fn a_programme_chooser_offers_the_names_the_web_ui_shows() {
    const KEY: &str = "BSH.Common.Root.SelectedProgram";

    let labels = embedded().for_device("BOSCH", "SMD8TCX01E");
    let (mapping, configs) = dishwasher_mapping_and_configs();
    let entry = mapping
        .entries()
        .iter()
        .find(|entry| entry.key.as_deref() == Some(KEY))
        .expect("the dishwasher declares a programme chooser");

    // What the web UI shows for the same list: the `[programs]` table, one lookup per raw option.
    let expected: Vec<String> = entry
        .options
        .iter()
        .map(|program| labels.program(program).text)
        .collect();
    assert!(
        expected.iter().any(|label| label == "Eco 50 °C"),
        "premise: the dishwasher's programmes have contributed names — got {expected:?}"
    );

    assert_eq!(
        config_for_slot(&configs, "BSH_Common_Root_SelectedProgram").get("options"),
        Some(&serde_json::json!(expected)),
        "the programme chooser must offer Home Assistant the names the web UI shows, in the \
         appliance's own declared order"
    );
}

/// Home Assistant must be able to tell two options apart.
///
/// With two identical entries in a `select`, neither the operator nor the reverse map can say which
/// programme was meant. Naming creates this: the oven has programmes whose leaf names repeat
/// (`Dish.Automatic.Conv.Steam.UnstuffedDuck` and
/// `Dish.Recommendation.Conv.Steam.MeatProbe.UnstuffedDuck`). Swept across all four appliances.
#[test]
fn no_entity_offers_home_assistant_the_same_option_twice() {
    let mut checked = 0_usize;
    for fixture in FIXTURES {
        for config in &mapping_and_configs(fixture).1 {
            checked += usize::from(assert_options_are_distinct(fixture, config));
        }
    }
    assert!(
        checked > 20,
        "only {checked} entities carry options — this sweep is only worth having over a real \
         surface"
    );
}

/// Asserts one config's `options` list names each choice once. Returns whether it had one at all.
fn assert_options_are_distinct(
    fixture: &str,
    config: &hc_core::discovery::DiscoveryConfig,
) -> bool {
    let Some(options) = config
        .payload
        .get("options")
        .and_then(|list| list.as_array())
    else {
        return false;
    };
    let mut seen = std::collections::BTreeSet::new();
    let duplicates: Vec<&serde_json::Value> = options
        .iter()
        .filter(|option| !seen.insert(option.to_string()))
        .collect();
    assert!(
        duplicates.is_empty(),
        "{fixture}: `{}` offers {} option(s) more than once, and neither an operator nor the \
         reverse map can tell them apart: {duplicates:#?}",
        config.id,
        duplicates.len()
    );
    true
}

/// The four real appliance descriptions.
const FIXTURES: [&str; 4] = [
    "dishwasher-tls.zip",
    "fridge-tls.zip",
    "hob-aes.zip",
    "oven-tls.zip",
];

/// Builds one fixture's mapping and its Home Assistant configs through the real catalogue.
///
/// The mapping is returned too, so a test can state its expectation without a literal. The brand
/// and `vib` come from the profile's manifest, so the model layer is the one the server would pick.
fn mapping_and_configs(
    fixture: &str,
) -> (
    hc_core::mapping::DeviceMapping,
    Vec<hc_core::discovery::DiscoveryConfig>,
) {
    let bytes = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/hc-profile/tests/fixtures")
            .join(fixture),
    )
    .unwrap_or_else(|err| panic!("reading {fixture}: {err}"));
    let profile = hc_profile::Profile::from_zip_bytes(&bytes)
        .unwrap_or_else(|err| panic!("parsing {fixture}: {err}"));
    let identity = profile.identity();
    let catalogue = server::mqtt::CatalogueNames::for_device(&identity.brand, &identity.vib);
    let device = hc_core::mapping::DeviceId::new("d");
    let mapping = hc_core::mapping::DeviceMapping::build(
        &device,
        profile.model(),
        &hc_core::mapping::Excludes::none(),
    );
    let info = hc_core::discovery::DeviceInfo::default();
    let configs = hc_core::discovery::discovery_configs(
        "kessel",
        &device,
        &mapping,
        &hc_core::discovery::Announcement {
            mode: hc_core::discovery::DiscoveryMode::Full,
            info: &info,
            names: &catalogue,
            origin: &hc_core::discovery::DiscoveryOrigin::KESSEL,
        },
    );
    (mapping, configs)
}

/// Builds the dishwasher's mapping and its Home Assistant configs through the real catalogue.
fn dishwasher_mapping_and_configs() -> (
    hc_core::mapping::DeviceMapping,
    Vec<hc_core::discovery::DiscoveryConfig>,
) {
    mapping_and_configs("dishwasher-tls.zip")
}

/// Builds the fridge's Home Assistant configs through the real contributed catalogue.
fn fridge_configs() -> Vec<hc_core::discovery::DiscoveryConfig> {
    mapping_and_configs("fridge-tls.zip").1
}

/// The published config whose topic slot is `slot`.
fn config_for_slot<'c>(
    configs: &'c [hc_core::discovery::DiscoveryConfig],
    slot: &str,
) -> &'c serde_json::Value {
    let Some(config) = configs.iter().find(|config| config.id == slot) else {
        panic!("no discovery config for {slot}")
    };
    &config.payload
}

/// The catalogue decides which entities are prominent, because no rule can.
///
/// `mapping::PRIMARY_LEAVES` is a short list of leaf names; everything else arrives
/// `enabled_by_default: false` under `config` or `diagnostic`. That buries the fridge's eco, fresh,
/// sabbath and vacation modes and its lights, so the catalogue can promote entities, and this
/// asserts that reaches Home Assistant. `entity_category = "primary"` means no category at all.
#[test]
fn the_catalogue_can_promote_a_feature_the_heuristic_buries() {
    let configs = fridge_configs();

    for slot in [
        "Refrigeration_Common_Setting_EcoMode",
        "Refrigeration_Common_Setting_FreshMode",
        "Refrigeration_Common_Setting_SabbathMode",
        "Refrigeration_Common_Setting_VacationMode",
        "Refrigeration_Common_Setting_Light_Internal_Power",
        "Refrigeration_Common_Setting_Light_External_Power",
    ] {
        let payload = config_for_slot(&configs, slot);
        assert_eq!(
            payload.get("enabled_by_default"),
            None,
            "{slot}: a contributed `enabled_by_default = true` means the key is omitted, which is \
             how HA reads \"enabled\""
        );
        assert_eq!(
            payload.get("entity_category"),
            None,
            "{slot}: `entity_category = \"primary\"` puts it in HA's prominent section, not under \
             Configuration or Diagnostic"
        );
    }

    // The override is per key, not a blanket promotion: a setting nobody promoted stays buried.
    let brightness = config_for_slot(
        &configs,
        "Refrigeration_Common_Setting_Light_Internal_Brightness",
    );
    assert_eq!(
        brightness.get("enabled_by_default"),
        Some(&serde_json::Value::Bool(false)),
        "a feature the catalogue does not promote keeps the heuristic's answer"
    );
}

/// A name reaches the entity's `name` and nothing else.
///
/// This keeps a contributed name safe to correct or translate: if it fed the entity id or the
/// `unique_id`, fixing a typo would rename entities and break automations. `hc-core` pins the same
/// from its side; this pins it against the real catalogue.
#[test]
fn a_catalogue_name_never_reaches_an_identifier() {
    let catalogue = server::mqtt::CatalogueNames::for_device("BOSCH", "SMD8TCX01E");
    let profile = {
        let bytes = std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/hc-profile/tests/fixtures/dishwasher-tls.zip"),
        )
        .expect("the dishwasher fixture is readable");
        hc_profile::Profile::from_zip_bytes(&bytes).expect("the dishwasher fixture parses")
    };
    let device = hc_core::mapping::DeviceId::new("dishwasher");
    let mapping = hc_core::mapping::DeviceMapping::build(
        &device,
        profile.model(),
        &hc_core::mapping::Excludes::none(),
    );
    let info = hc_core::discovery::DeviceInfo::default();
    let named = hc_core::discovery::discovery_configs(
        "kessel",
        &device,
        &mapping,
        &hc_core::discovery::Announcement {
            mode: hc_core::discovery::DiscoveryMode::Full,
            info: &info,
            names: &catalogue,
            origin: &hc_core::discovery::DiscoveryOrigin::KESSEL,
        },
    );
    let unnamed = hc_core::discovery::discovery_configs(
        "kessel",
        &device,
        &mapping,
        &hc_core::discovery::Announcement::bare(hc_core::discovery::DiscoveryMode::Full),
    );

    assert_eq!(
        named.len(),
        unnamed.len(),
        "a catalogue must not add or drop a config"
    );
    let mut renamed = 0_usize;
    for (with, without) in named.iter().zip(unnamed.iter()) {
        assert_eq!(with.id, without.id, "a catalogue must not move a component");
        // `default_entity_id` is the key emitted; comparing an absent key would compare `None`
        // with `None` and pin nothing.
        for key in [
            "unique_id",
            "default_entity_id",
            "state_topic",
            "command_topic",
        ] {
            assert_eq!(
                with.payload.get(key),
                without.payload.get(key),
                "{}: `{key}` must not depend on what a feature is called",
                with.id
            );
        }
        if with.payload.get("name") != without.payload.get("name") {
            renamed += 1;
        }
    }
    assert!(
        renamed > 0,
        "no name changed, so this test proved nothing about the catalogue being wired in"
    );
}

/// Every contributed icon is an `mdi:` name; anything else is dropped at announce time, which would
/// be a silent typo.
#[test]
fn every_contributed_icon_is_a_material_design_icon_name() {
    for fixture in FIXTURES {
        let (mapping, _) = mapping_and_configs(fixture);
        let catalogue = catalogue_for(fixture);
        for key in mapping
            .entries()
            .iter()
            .filter_map(|entry| entry.key.as_deref())
        {
            let Some(icon) = hc_core::discovery::FeatureNaming::icon(&catalogue, key) else {
                continue;
            };
            let name = icon.strip_prefix("mdi:").unwrap_or_default();
            assert!(
                !name.is_empty()
                    && name
                        .chars()
                        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'),
                "{fixture}: {key}: `{icon}` is not `mdi:<name>`"
            );
        }
    }
}

/// A contributed `device_class` reaches Home Assistant. One that does not fit the platform its
/// feature lands on is dropped to save the entity, so here it fails instead.
#[test]
fn every_contributed_device_class_fits_the_platform_it_lands_on() {
    let mut checked = 0_usize;
    for fixture in FIXTURES {
        let (mapping, configs) = mapping_and_configs(fixture);
        let catalogue = catalogue_for(fixture);
        for entry in mapping.entries() {
            let Some(key) = entry.key.as_deref() else {
                continue;
            };
            let Some(class) = hc_core::discovery::FeatureNaming::device_class(&catalogue, key)
            else {
                continue;
            };
            let Some(config) = configs
                .iter()
                .find(|config| config.id == key.replace('.', "_"))
            else {
                continue;
            };
            assert_eq!(
                config
                    .payload
                    .get("device_class")
                    .and_then(|value| value.as_str()),
                Some(class.as_str()),
                "{fixture}: {key}: `{class}` is not a `{}` device class",
                config.component
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "no contributed device class was checked");
}

/// The faults an operator automates on arrive as problems, and the door carries its icon.
#[test]
fn the_dishwasher_reports_its_faults_as_problems() {
    let (_, configs) = dishwasher_mapping_and_configs();
    let field = |slot: &str, key: &str| {
        config_for_slot(&configs, slot)
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::to_owned)
    };
    assert_eq!(
        field("Dishcare_Dishwasher_Event_SaltLack", "device_class").as_deref(),
        Some("problem")
    );
    assert_eq!(
        field("BSH_Common_Status_DoorState", "icon").as_deref(),
        Some("mdi:door")
    );
}

/// The catalogue for a fixture's appliance.
fn catalogue_for(fixture: &str) -> server::mqtt::CatalogueNames {
    let bytes = std::fs::read(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/hc-profile/tests/fixtures")
            .join(fixture),
    )
    .unwrap_or_else(|err| panic!("reading {fixture}: {err}"));
    let profile = hc_profile::Profile::from_zip_bytes(&bytes)
        .unwrap_or_else(|err| panic!("parsing {fixture}: {err}"));
    let identity = profile.identity();
    server::mqtt::CatalogueNames::for_device(&identity.brand, &identity.vib)
}

/// The committed `schema.json` editors complete from is what the loader's types describe now.
/// `just types` regenerates it.
#[test]
fn the_committed_catalogue_schema_is_current() {
    let committed = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data/labels/schema.json"),
    )
    .expect("data/labels/schema.json is committed");
    assert!(
        committed == server::labels::catalogue_schema(),
        "data/labels/schema.json is stale: run `just types` and commit the result"
    );
}
