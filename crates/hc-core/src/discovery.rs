//! Home Assistant MQTT discovery, in `full` or `curated` mode.
//!
//! Each appliance is one retained device-based discovery message
//! (`homeassistant/device/kessel_<slug>/config`): the `device` and `origin` blocks and the
//! availability topic once, and one entry per entity under `components`. Both modes cover the same
//! raw state/set topics ([`crate::bridge`] publishes those regardless of mode); the mode only
//! decides which features get a component. `full` announces every surfaceable feature, `curated`
//! only the cleanly-classifiable ones. Every topic and identifier is built from the [`DeviceId`]
//! slug, never from an appliance identifier.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::mapping::{DeviceId, DeviceMapping, EntityClass, EntityMapping};
use crate::presentation::{self, PresentedEntity};

/// Which discovery surface to announce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryMode {
    /// A discovery config for **every** surfaceable feature.
    Full,
    /// A narrowed discovery surface for a tidy HA UI — coverage of raw topics is unchanged.
    Curated,
}

/// The appliance's non-secret identity, for the HA `device` block.
///
/// Brand and model are not secret; serial, MAC and `haId` are, and have no field here. The caller
/// holds the profile and fills this in; an absent field is omitted from the payload. Not
/// `#[non_exhaustive]`, so callers can construct it with a struct expression.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceInfo {
    /// `<brand>`, for example `BOSCH`, `NEFF` or `SIEMENS`. Becomes HA's `manufacturer`.
    pub brand: Option<String>,
    /// `vib`, the model designation, for example `HRG7784B1`. Becomes HA's `model`.
    pub model: Option<String>,
    /// The device-description version, when the profile carries one. Becomes HA's `sw_version`.
    ///
    /// This is the description version, not a firmware version: the appliance does not report its
    /// firmware over this protocol.
    pub sw_version: Option<String>,
    /// Where the appliance is managed, for the device page's "Visit" link. Becomes HA's
    /// `configuration_url`; `homeassistant://` links open inside Home Assistant.
    pub configuration_url: Option<String>,
}

/// The software publishing the discovery message. Home Assistant requires it on device-based
/// discovery and shows it on the device page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryOrigin {
    /// The application's name.
    pub name: &'static str,
    /// Its version, when known.
    pub sw_version: Option<&'static str>,
    /// Where to get help with it.
    pub support_url: Option<&'static str>,
}

impl DiscoveryOrigin {
    /// Kessel, with no version or support link.
    pub const KESSEL: Self = Self {
        name: "Kessel",
        sw_version: None,
        support_url: None,
    };
}

/// Where a feature's human-readable names come from.
///
/// The server implements this over its label catalogue so the web UI and Home Assistant show the
/// same names; [`DerivedNames`] is the implementation for a caller with no catalogue. Names are
/// presentation only: they never reach a `unique_id`, entity id, topic or write payload, so they
/// can change without breaking HA entities or automations.
///
/// `Send + Sync` because an [`Announcement`] is held across an `await` in a spawned task; `Debug`
/// because [`crate::bridge::DeviceBridge`] holds one.
pub trait FeatureNaming: Send + Sync + core::fmt::Debug {
    /// The display name for a dotted feature key, or [`None`] to take the derived fallback.
    fn feature_name(&self, key: &str) -> Option<String>;

    /// Contributed help text for a feature, or [`None`] when nobody has described it.
    ///
    /// Never derived: unlike [`Self::feature_name`], [`None`] here means "say nothing", not "fall
    /// back". Defaults to [`None`].
    fn feature_description(&self, key: &str) -> Option<String> {
        let _ = key;
        None
    }

    /// The display name for one enum member of a feature, keyed by the member's raw name.
    ///
    /// [`None`] means the raw member is shown. Writes always carry the raw member. Defaults to
    /// [`None`].
    fn member_name(&self, key: &str, member: &str) -> Option<String> {
        let _ = (key, member);
        None
    }

    /// The display name for a programme, keyed by its dotted feature key.
    ///
    /// Programmes are a separate namespace from features in the catalogue. Defaults to [`None`].
    fn program_name(&self, key: &str) -> Option<String> {
        let _ = key;
        None
    }

    /// The catalogue's say in how prominently a feature is shown.
    ///
    /// Decides whether Home Assistant enables the entity and which section of the device page it
    /// lands in (see [`crate::presentation::Curation`]). Defaults to no override, which leaves
    /// [`crate::mapping`]'s heuristic in charge.
    fn curation(&self, key: &str) -> crate::presentation::Curation {
        let _ = key;
        crate::presentation::Curation::default()
    }

    /// The catalogue's icon for a feature (`mdi:<name>`), or [`None`] for Home Assistant's own.
    fn icon(&self, key: &str) -> Option<String> {
        let _ = key;
        None
    }

    /// The catalogue's `device_class` for a feature, overriding the one derived from its unit.
    ///
    /// A class outside the entity's platform vocabulary is dropped, never published.
    fn device_class(&self, key: &str) -> Option<String> {
        let _ = key;
        None
    }
}

/// The [`FeatureNaming`] for a caller with no catalogue: every key takes the derived fallback.
#[derive(Debug, Clone, Copy, Default)]
pub struct DerivedNames;

impl FeatureNaming for DerivedNames {
    fn feature_name(&self, _key: &str) -> Option<String> {
        None
    }
}

/// Everything a discovery announcement needs beyond the mapping: how wide it is, what the appliance
/// is, and what its features are called.
pub struct Announcement<'a> {
    /// How wide the discovery surface is. Raw topic coverage is unaffected.
    pub mode: DiscoveryMode,
    /// The appliance's non-secret identity, for the HA `device` block.
    pub info: &'a DeviceInfo,
    /// Where a feature's display name comes from.
    pub names: &'a dyn FeatureNaming,
    /// Who is publishing, for the `origin` block.
    pub origin: &'a DiscoveryOrigin,
}

impl core::fmt::Debug for Announcement<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // `names` is left out; it adds nothing useful to a debug line.
        f.debug_struct("Announcement")
            .field("mode", &self.mode)
            .field("info", self.info)
            .field("origin", self.origin)
            .finish_non_exhaustive()
    }
}

impl Announcement<'_> {
    /// An announcement with no appliance identity and no name catalogue.
    #[must_use]
    pub const fn bare(mode: DiscoveryMode) -> Self {
        Self {
            mode,
            info: &EMPTY_INFO,
            names: &DerivedNames,
            origin: &DiscoveryOrigin::KESSEL,
        }
    }
}

/// The empty identity [`Announcement::bare`] borrows. A `const fn` cannot allocate, and
/// `DeviceInfo::default()` is not `const`.
static EMPTY_INFO: DeviceInfo = DeviceInfo {
    brand: None,
    model: None,
    sw_version: None,
    configuration_url: None,
};

/// One entity of the device discovery message: its key under `components`, its HA platform, and
/// its config.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiscoveryConfig {
    /// The key under `components`: the feature key with `.`→`_`, or a uid fallback.
    pub id: String,
    /// The HA platform/component token (`switch`/`select`/`sensor`/…).
    pub component: &'static str,
    /// The component's config, `platform` included. The shared `device`, `origin` and availability
    /// keys sit at the root of the device message instead.
    pub payload: Value,
}

/// The HA component token for an [`EntityClass`].
const fn component_of(class: EntityClass) -> &'static str {
    match class {
        EntityClass::Switch => "switch",
        EntityClass::Select => "select",
        EntityClass::Sensor => "sensor",
        EntityClass::BinarySensor => "binary_sensor",
        EntityClass::Number => "number",
        EntityClass::Button => "button",
    }
}

/// Whether an entry is surfaceable as an HA entity: it has a state or command topic and is
/// addressable ([`crate::mapping::is_addressable`]).
///
/// Lists, programme groups, programme definitions and protection ports carry a `uid` and an
/// `access` word but report nothing; announcing them gives sensors that read `unknown` for ever.
/// They keep their raw topics.
const fn is_surfaceable(entry: &EntityMapping) -> bool {
    crate::mapping::is_addressable(entry.kind)
        && (entry.state_topic.is_some() || entry.command_topic.is_some())
}

/// Whether an entry belongs in the **curated** discovery surface.
///
/// Curation trims presentation, never coverage. Only the generic fallback `Sensor` with no
/// `device_class` is held out; it keeps its raw state topic.
const fn is_curated(entry: &EntityMapping) -> bool {
    match entry.class {
        EntityClass::Sensor => entry.device_class.is_some(),
        _ => true,
    }
}

/// The full device topic for a device-relative path: `<base>/<device>/<relative>`.
fn device_topic(base: &str, device: &DeviceId, relative: &str) -> String {
    format!("{base}/{}/{relative}", device.as_str())
}

/// The component id for an entry: the feature key with `.`→`_`, or a uid fallback.
///
/// It was also the object id of the per-entity topics earlier versions published, which
/// [`is_legacy_config`] recognises.
fn config_slot(entry: &EntityMapping) -> String {
    entry.key.as_deref().map_or_else(
        || format!("uid_{:04X}", entry.uid),
        |key| key.replace('.', "_"),
    )
}

/// The HA `default_entity_id`: `<component>.<device>_<feature key>`, slugified.
///
/// Published so the entity id comes from the feature key rather than the display name: names may
/// change, but entity ids are used in automations and dashboards. The whole dotted key is used
/// because two features can share a leaf name, and HA would suffix a colliding id with `_2`.
/// HA applies it only when it creates an entity. Requires HA 2025.10 or later, which replaced the
/// deprecated `object_id` with this key; older versions discard the config.
fn default_entity_id(component: &str, device: &DeviceId, entry: &EntityMapping) -> String {
    let key = entry
        .key
        .clone()
        .unwrap_or_else(|| format!("uid_{:04X}", entry.uid));
    format!(
        "{component}.{}",
        slugify(&format!("{}_{key}", device.as_str()))
    )
}

/// Lower-cases and reduces every run of non-alphanumeric characters to a single `_`.
///
/// HA would otherwise rewrite the id by its own rules; doing it here makes the published id and
/// the resulting entity id the same string. ASCII-only: a non-ASCII character collapses to `_`
/// (`Küche` becomes `k_che`) rather than being transliterated.
fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_underscore = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.extend(ch.to_lowercase());
            pending_underscore = false;
        } else if !pending_underscore {
            out.push('_');
            pending_underscore = true;
        }
    }
    out.trim_matches('_').to_owned()
}

/// The prefix on the HA device identifier.
///
/// A literal rather than the configurable topic root, so editing the topic root does not make HA
/// treat the appliance as a new device. Prefixed so a slug like `dishwasher` cannot collide with
/// another integration's device identifier.
const DEVICE_ID_PREFIX: &str = "kessel_";

/// The HA `device` object every config on this appliance shares.
///
/// Without a `device`, HA registers each config as a standalone entity and shows no device.
/// `manufacturer`, `model` and `sw_version` come from [`DeviceInfo`] and are omitted, never
/// filled with a placeholder, when absent. There is deliberately no `serial_number` or
/// `connections` (a MAC): topics and payloads never carry appliance identifiers.
fn device_block(device: &DeviceId, info: &DeviceInfo) -> Value {
    let mut map = Map::new();
    map.insert(
        "identifiers".to_owned(),
        Value::Array(vec![Value::String(format!(
            "{DEVICE_ID_PREFIX}{}",
            device.as_str()
        ))]),
    );
    map.insert("name".to_owned(), Value::String(device_name(device, info)));
    insert_optional(&mut map, "manufacturer", info.brand.as_deref());
    insert_optional(&mut map, "model", info.model.as_deref());
    insert_optional(&mut map, "sw_version", info.sw_version.as_deref());
    insert_optional(
        &mut map,
        "configuration_url",
        info.configuration_url.as_deref(),
    );
    Value::Object(map)
}

/// The HA `origin` object.
fn origin_block(origin: &DiscoveryOrigin) -> Value {
    let mut map = Map::new();
    map.insert("name".to_owned(), Value::String(origin.name.to_owned()));
    insert_optional(&mut map, "sw_version", origin.sw_version);
    insert_optional(&mut map, "support_url", origin.support_url);
    Value::Object(map)
}

/// What Home Assistant shows this device as: `"<BRAND> - <VIB>"` when both are known, otherwise
/// the operator's slug.
///
/// HA prefixes the device name onto every entity's `friendly_name`, so the model designation
/// identifies the appliance better than the slug. The name is presentation only; the registry
/// identity stays [`DEVICE_ID_PREFIX`] plus the slug.
fn device_name(device: &DeviceId, info: &DeviceInfo) -> String {
    let part = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    match (part(&info.brand), part(&info.model)) {
        (Some(brand), Some(model)) => format!("{brand} - {model}"),
        // A brand alone does not say which appliance this is.
        _ => device.as_str().to_owned(),
    }
}

/// Inserts `key` only when there is a non-empty value for it.
///
/// `hc_profile` records an undecodable `<brand>` as an empty string, which HA would show as a
/// blank row.
fn insert_optional(map: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        map.insert(key.to_owned(), Value::String(value.to_owned()));
    }
}

/// Keeps a `sensor` state within Home Assistant's 255-character limit.
///
/// HA refuses a longer state outright and shows `unknown`. The clamp applies to HA's view only;
/// the raw state topic still carries the full value. Shorter values, including
/// [`crate::bridge::PAYLOAD_NONE`], pass through unchanged.
const STATE_CLAMP_TEMPLATE: &str = "{{ value[:255] }}";

/// The two payloads a boolean feature's state topic actually carries.
///
/// [`crate::bridge::rendered_value`] renders a bool as raw JSON, so the state is `true` / `false`.
/// HA publishes `payload_on` verbatim to the command topic, where it parses back as a JSON bool;
/// HA's defaults `"ON"` / `"OFF"` would arrive as strings.
const BOOLEAN_ON: &str = "true";
/// The `false` half of [`BOOLEAN_ON`].
const BOOLEAN_OFF: &str = "false";

/// The `<event>` enumeration's member names: `Off` / `Present` / `Confirmed`.
///
/// An event's state topic carries the label. A binary sensor has only two states, so a
/// `Confirmed` payload matches neither and HA keeps the previous state.
const EVENT_PRESENT: &str = "Present";
/// The `off` half of [`EVENT_PRESENT`].
const EVENT_OFF: &str = "Off";

/// Whether this entry is announced as an HA `button`.
const fn is_button(entry: &EntityMapping) -> bool {
    matches!(entry.class, EntityClass::Button)
}

/// Inserts the on/off/press payloads the entry's platform needs to match its own state topic.
///
/// HA defaults these to `"ON"` / `"OFF"` / `"PRESS"`, which no Kessel topic carries: a `switch`
/// or `binary_sensor` would read `unknown` for ever, and a button would write the string `"PRESS"`.
fn insert_platform_payloads(map: &mut Map<String, Value>, entry: &EntityMapping) {
    match entry.class {
        EntityClass::Switch => {
            map.insert(
                "payload_on".to_owned(),
                Value::String(BOOLEAN_ON.to_owned()),
            );
            map.insert(
                "payload_off".to_owned(),
                Value::String(BOOLEAN_OFF.to_owned()),
            );
        }
        EntityClass::BinarySensor => {
            let (on, off) = binary_sensor_payloads(entry);
            map.insert("payload_on".to_owned(), Value::String(on.to_owned()));
            map.insert("payload_off".to_owned(), Value::String(off.to_owned()));
        }
        EntityClass::Button => {
            // A command is executed by writing `true` to its feature.
            map.insert(
                "payload_press".to_owned(),
                Value::String(BOOLEAN_ON.to_owned()),
            );
        }
        EntityClass::Select | EntityClass::Sensor | EntityClass::Number => {}
    }
}

/// The on/off payloads for a `binary_sensor`:
///
/// | source | state topic carries | payloads |
/// |---|---|---|
/// | a plain boolean | `true` / `false` | [`BOOLEAN_ON`] / [`BOOLEAN_OFF`] |
/// | an `Off`/`On` enumeration | the label | `On` / `Off` |
/// | an `<event>` | `Off` / `Present` / `Confirmed` | [`EVENT_PRESENT`] / [`EVENT_OFF`] |
///
/// An entity without declared members publishes raw JSON, so no options means a boolean. Member
/// names are matched literally because appliances declare the same pair in different orders.
fn binary_sensor_payloads(entry: &EntityMapping) -> (&'static str, &'static str) {
    // No declared members: a boolean, published as raw JSON.
    if entry.options.is_empty() {
        return (BOOLEAN_ON, BOOLEAN_OFF);
    }
    let has = |name: &str| entry.options.iter().any(|option| option == name);
    if entry.options.len() == 2 && has(EVENT_OFF) && has("On") {
        return ("On", EVENT_OFF);
    }
    (EVENT_PRESENT, EVENT_OFF)
}

/// Builds the discovery payload for one entry, under `announcement`.
fn payload_for(
    base: &str,
    device: &DeviceId,
    entry: &EntityMapping,
    announcement: &Announcement<'_>,
) -> Value {
    // The same presentation the dashboard uses; see `crate::presentation`.
    let presented = presentation::present(entry, announcement.names);
    let mut map = Map::new();
    insert_identity(
        &mut map,
        &Identity {
            component: component_of(entry.class),
            device,
            entry,
            presented: &presented,
        },
    );
    insert_topics(&mut map, base, device, entry);
    insert_platform_payloads(&mut map, entry);
    insert_presentation(&mut map, entry, &presented, announcement.names);
    match entry.number_range {
        Some(range) => insert_number_range(&mut map, range),
        None if entry.class == EntityClass::Number => insert_unstated_number_range(&mut map),
        None => {}
    }
    Value::Object(map)
}

/// Who an entity is.
struct Identity<'i> {
    component: &'i str,
    device: &'i DeviceId,
    entry: &'i EntityMapping,
    presented: &'i PresentedEntity,
}

/// Who this entity is.
fn insert_identity(map: &mut Map<String, Value>, identity: &Identity<'_>) {
    let Identity {
        component,
        device,
        entry,
        presented,
    } = *identity;
    map.insert(
        "unique_id".to_owned(),
        Value::String(entry.unique_id.clone()),
    );
    map.insert("name".to_owned(), Value::String(presented.label.clone()));
    // So the entity id comes from the key, not the name; see `default_entity_id`.
    map.insert(
        "default_entity_id".to_owned(),
        Value::String(default_entity_id(component, device, entry)),
    );
}

/// Where this entity reads and writes. Availability is shared, at the root of the device message.
fn insert_topics(
    map: &mut Map<String, Value>,
    base: &str,
    device: &DeviceId,
    entry: &EntityMapping,
) {
    // HA's button schema does not accept a `state_topic`, even when the appliance declares the
    // command readable.
    if let Some(state) = entry.state_topic.as_deref().filter(|_| !is_button(entry)) {
        map.insert(
            "state_topic".to_owned(),
            Value::String(device_topic(base, device, state)),
        );
        // Only a `sensor` carries free-form text that can exceed HA's state length limit.
        if entry.class == EntityClass::Sensor {
            map.insert(
                "value_template".to_owned(),
                Value::String(STATE_CLAMP_TEMPLATE.to_owned()),
            );
        }
    }
    // Only platforms with a write path get a `command_topic`. The raw `…/set` topic is still
    // subscribed for every writable feature.
    if let Some(command) = entry.command_topic.as_deref()
        && matches!(
            entry.class,
            EntityClass::Switch | EntityClass::Select | EntityClass::Number | EntityClass::Button
        )
    {
        map.insert(
            "command_topic".to_owned(),
            Value::String(device_topic(base, device, command)),
        );
    }
}

/// Whether the announce gives Home Assistant an `options` list for this entity, against which HA
/// then validates every state.
///
/// Shared with [`DeviceBridge::publish_value`](crate::bridge::DeviceBridge::publish_value), which must know which entities HA holds to a
/// vocabulary: HA silently discards a state outside the list.
///
/// - `select`: always. HA rejects a select config without `options`, so an appliance with no
///   programmes gets an empty list.
/// - `sensor`: only when it has options, which imply `device_class: enum`.
/// - every other platform: never.
#[must_use]
pub const fn announces_options(entry: &EntityMapping) -> bool {
    match entry.class {
        EntityClass::Select => true,
        EntityClass::Sensor => !entry.options.is_empty(),
        _ => false,
    }
}

/// How Home Assistant should render and file this entity: its options, class, unit, statistics,
/// device-page section and whether it arrives enabled.
fn insert_presentation(
    map: &mut Map<String, Value>,
    entry: &EntityMapping,
    presented: &PresentedEntity,
    names: &dyn FeatureNaming,
) {
    // The presented member names, in the appliance's declared order. This list, the state that
    // `bridge::publish_value` publishes and what `bridge::handle_set` accepts all come from
    // `PresentedEntity`, so they agree; HA silently stops updating the entity otherwise.
    if announces_options(entry) {
        map.insert(
            "options".to_owned(),
            Value::Array(
                entry
                    .options
                    .iter()
                    .map(|member| Value::String(presented.member_label(member).to_owned()))
                    .collect(),
            ),
        );
    }
    let key = entry.key.as_deref();
    // The catalogue's class wins over the one derived from the unit. The derived classes are
    // sensor classes, which a binary sensor's vocabulary does not share.
    let device_class = key.and_then(|key| names.device_class(key)).or_else(|| {
        entry
            .device_class
            .filter(|_| entry.class != EntityClass::BinarySensor)
            .map(|class| class.as_str().to_owned())
    });
    if let Some(device_class) = device_class {
        map.insert("device_class".to_owned(), Value::String(device_class));
    }
    if let Some(icon) = key
        .and_then(|key| names.icon(key))
        .filter(|icon| icon.starts_with("mdi:"))
    {
        map.insert("icon".to_owned(), Value::String(icon));
    }
    if let Some(unit) = entry.unit {
        map.insert(
            "unit_of_measurement".to_owned(),
            Value::String(unit.to_owned()),
        );
    }
    if let Some(state_class) = entry.state_class {
        map.insert(
            "state_class".to_owned(),
            Value::String(state_class.as_str().to_owned()),
        );
    }
    // From `presented`, not `entry`: the catalogue may override either.
    if let Some(category) = presented.entity_category {
        map.insert(
            "entity_category".to_owned(),
            Value::String(category.as_str().to_owned()),
        );
    }
    // Emitted only to disable; HA's default is enabled.
    if !presented.enabled_by_default {
        map.insert("enabled_by_default".to_owned(), Value::Bool(false));
    }
}

/// How far a `number` reaches when the appliance stated no bounds of its own.
///
/// HA's defaults (`0`–`100`) would refuse valid values such as a freezer at -18 °C, so the range
/// is wide enough that only the appliance refuses a value. Paired with `mode: "box"` so no slider
/// is drawn across it.
const UNSTATED_BOUND: f64 = 100_000.0_f64;

/// Inserts the range for a `number` whose appliance declared none it could be controlled over.
///
/// `min`/`max` are [`UNSTATED_BOUND`], `mode` is `box` because there is no meaningful span for a
/// slider, and `step` is omitted rather than claiming a granularity the appliance never stated.
fn insert_unstated_number_range(map: &mut Map<String, Value>) {
    map.insert("min".to_owned(), Value::from(-UNSTATED_BOUND));
    map.insert("max".to_owned(), Value::from(UNSTATED_BOUND));
    map.insert("mode".to_owned(), Value::from("box"));
}

/// Inserts a number's `min`/`max`/`step`, each only when the entity declared it.
fn insert_number_range(map: &mut Map<String, Value>, range: hc_profile::NumericRange) {
    if let Some(min) = range.min {
        map.insert("min".to_owned(), Value::from(min));
    }
    if let Some(max) = range.max {
        map.insert("max".to_owned(), Value::from(max));
    }
    if let Some(step) = range.step {
        map.insert("step".to_owned(), Value::from(step));
    }
}

/// Strips every key the target platform does not accept. Runs last, over the finished payload.
///
/// HA discards the whole config when a key or `device_class` does not belong to the platform, so
/// one misplaced field means a missing entity. A single final pass also covers keys added later.
fn sanitize_for_platform(map: &mut Map<String, Value>, component: &str) {
    // `unit_of_measurement` is sensor/number only; `state_class` sensor only.
    if !matches!(component, "sensor" | "number") {
        map.remove("unit_of_measurement");
    }
    if component != "sensor" {
        map.remove("state_class");
    }
    // `min`/`max`/`step`/`mode` are number only.
    if component != "number" {
        map.remove("min");
        map.remove("max");
        map.remove("step");
        map.remove("mode");
    }
    // `options` is select and enum-sensor only.
    if !matches!(component, "sensor" | "select") {
        map.remove("options");
    }
    // A `device_class` value from another platform's vocabulary is refused like a misplaced key.
    if !map
        .get("device_class")
        .and_then(Value::as_str)
        .is_none_or(|class| platform_takes_device_class(component, class))
    {
        map.remove("device_class");
    }
    // On a sensor, HA requires `device_class: enum` and `options` together, so either half alone
    // is dropped rather than the other being synthesised.
    let is_enum = map.get("device_class") == Some(&Value::String("enum".to_owned()));
    if component == "sensor" && is_enum && !map.contains_key("options") {
        map.remove("device_class");
    }
    if component == "sensor" && !is_enum {
        map.remove("options");
    }
}

/// Home Assistant's `binary_sensor` device classes.
pub const BINARY_SENSOR_CLASSES: [&str; 28] = [
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
];

/// Home Assistant's `sensor` device classes.
pub const SENSOR_CLASSES: [&str; 59] = [
    "absolute_humidity",
    "apparent_power",
    "aqi",
    "area",
    "atmospheric_pressure",
    "battery",
    "blood_glucose_concentration",
    "carbon_dioxide",
    "carbon_monoxide",
    "conductivity",
    "current",
    "data_rate",
    "data_size",
    "date",
    "distance",
    "duration",
    "energy",
    "energy_distance",
    "energy_storage",
    "enum",
    "frequency",
    "gas",
    "humidity",
    "illuminance",
    "irradiance",
    "moisture",
    "monetary",
    "nitrogen_dioxide",
    "nitrogen_monoxide",
    "nitrous_oxide",
    "ozone",
    "ph",
    "pm1",
    "pm10",
    "pm25",
    "pm4",
    "power",
    "power_factor",
    "precipitation",
    "precipitation_intensity",
    "pressure",
    "reactive_energy",
    "reactive_power",
    "signal_strength",
    "sound_pressure",
    "speed",
    "sulphur_dioxide",
    "temperature",
    "timestamp",
    "volatile_organic_compounds",
    "volatile_organic_compounds_parts",
    "voltage",
    "volume",
    "volume_flow_rate",
    "volume_storage",
    "water",
    "weight",
    "wind_direction",
    "wind_speed",
];

/// Home Assistant's `button` device classes.
pub const BUTTON_CLASSES: [&str; 3] = ["identify", "restart", "update"];

/// Home Assistant's `switch` device classes.
pub const SWITCH_CLASSES: [&str; 2] = ["outlet", "switch"];

/// Every `device_class` any platform Kessel announces accepts, sorted and without repeats.
#[must_use]
pub fn known_device_classes() -> Vec<&'static str> {
    let mut classes: Vec<&'static str> = SENSOR_CLASSES
        .iter()
        .chain(&BINARY_SENSOR_CLASSES)
        .chain(&BUTTON_CLASSES)
        .chain(&SWITCH_CLASSES)
        .copied()
        .collect();
    classes.sort_unstable();
    classes.dedup();
    classes
}

/// Whether `class` is in `component`'s `device_class` vocabulary.
///
/// Each HA platform has its own `device_class` vocabulary, and HA refuses a config whose class is
/// from another one, so a contributed class that does not fit is dropped here. A `number` takes the
/// sensor classes except `enum`, `date` and `timestamp`. A positive list, so a new platform takes no
/// class by default.
#[must_use]
pub fn platform_takes_device_class(component: &str, class: &str) -> bool {
    match component {
        "sensor" => SENSOR_CLASSES.contains(&class),
        "number" => {
            SENSOR_CLASSES.contains(&class) && !matches!(class, "enum" | "date" | "timestamp")
        }
        "binary_sensor" => BINARY_SENSOR_CLASSES.contains(&class),
        "button" => BUTTON_CLASSES.contains(&class),
        "switch" => SWITCH_CLASSES.contains(&class),
        _ => false,
    }
}

/// Builds one discovery config for an entry, under `announcement`.
fn config_for(
    base: &str,
    device: &DeviceId,
    entry: &EntityMapping,
    announcement: &Announcement<'_>,
) -> DiscoveryConfig {
    let component = component_of(entry.class);
    let mut payload = payload_for(base, device, entry, announcement);
    // Last, over the finished payload.
    if let Value::Object(map) = &mut payload {
        sanitize_for_platform(map, component);
        map.insert("platform".to_owned(), Value::String(component.to_owned()));
    }
    DiscoveryConfig {
        id: config_slot(entry),
        component,
        payload,
    }
}

/// Builds the component configs for a device's mapping under `announcement`.
///
/// `full` returns one config per surfaceable feature; `curated` drops generic sensors with no
/// `device_class`. Synthetic controls are appended. Neither mode affects the raw state/set topics
/// published by [`crate::bridge`]. [`device_discovery`] wraps them into the message HA reads.
#[must_use]
pub fn discovery_configs(
    base: &str,
    device: &DeviceId,
    mapping: &DeviceMapping,
    announcement: &Announcement<'_>,
) -> Vec<DiscoveryConfig> {
    let mut configs: Vec<DiscoveryConfig> = mapping
        .entries()
        .iter()
        .filter(|entry| is_surfaceable(entry))
        .filter(|entry| announcement.mode == DiscoveryMode::Full || is_curated(entry))
        .map(|entry| config_for(base, device, entry, announcement))
        .collect();
    configs.extend(synthetic_controls(base, device, mapping));
    configs
}

/// The device-based discovery message for one appliance, and where it is published.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DeviceDiscovery {
    /// The retained topic: `homeassistant/device/kessel_<slug>/config`.
    pub config_topic: String,
    /// The message: `device`, `origin`, the shared availability keys, and `components`.
    pub payload: Value,
}

impl DeviceDiscovery {
    /// Each component's platform, by component id.
    #[must_use]
    pub fn platforms(&self) -> BTreeMap<String, String> {
        component_platforms(&self.payload)
    }

    /// The message with each of `gone` reduced to its platform alone, which is how Home Assistant
    /// is told to remove a component. A `gone` id that is also current is removed too, so a
    /// component that changed platform is retracted on its old one before it is announced again.
    #[must_use]
    pub fn retracting(&self, gone: &BTreeMap<String, String>) -> Value {
        let mut payload = self.payload.clone();
        if let Some(Value::Object(components)) = payload.get_mut("components") {
            for (id, platform) in gone {
                let mut stub = Map::new();
                stub.insert("platform".to_owned(), Value::String(platform.clone()));
                components.insert(id.clone(), Value::Object(stub));
            }
        }
        payload
    }
}

/// The device-based discovery topic for `device`.
///
/// The object id is the HA device identifier, so it does not move when the topic root does.
#[must_use]
pub fn device_config_topic(device: &DeviceId) -> String {
    format!(
        "homeassistant/device/{DEVICE_ID_PREFIX}{}/config",
        device.as_str()
    )
}

/// Builds the device-based discovery message for a device's mapping under `announcement`.
#[must_use]
pub fn device_discovery(
    base: &str,
    device: &DeviceId,
    mapping: &DeviceMapping,
    announcement: &Announcement<'_>,
) -> DeviceDiscovery {
    let mut root = Map::new();
    root.insert("device".to_owned(), device_block(device, announcement.info));
    root.insert("origin".to_owned(), origin_block(announcement.origin));
    // Shared by every component: HA merges these root keys into each one.
    root.insert(
        "availability_topic".to_owned(),
        Value::String(format!("{base}/{}/availability", device.as_str())),
    );
    root.insert(
        "payload_available".to_owned(),
        Value::String("online".to_owned()),
    );
    root.insert(
        "payload_not_available".to_owned(),
        Value::String("offline".to_owned()),
    );
    let components: Map<String, Value> = discovery_configs(base, device, mapping, announcement)
        .into_iter()
        .map(|config| (config.id, config.payload))
        .collect();
    root.insert("components".to_owned(), Value::Object(components));
    DeviceDiscovery {
        config_topic: device_config_topic(device),
        payload: Value::Object(root),
    }
}

/// Each component's platform in a device discovery message, by component id. Empty for anything
/// that is not one.
#[must_use]
pub fn component_platforms(payload: &Value) -> BTreeMap<String, String> {
    payload
        .get("components")
        .and_then(Value::as_object)
        .map(|components| {
            components
                .iter()
                .filter_map(|(id, config)| {
                    let platform = config.get("platform").and_then(Value::as_str)?;
                    Some((id.clone(), platform.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Subscribes every per-entity config earlier versions published for `device`:
/// `homeassistant/<component>/<slug>/<id>/config`.
///
/// The device message's own topic has one level fewer, so it does not match.
#[must_use]
pub fn legacy_config_filter(device: &DeviceId) -> String {
    format!("homeassistant/+/{}/+/config", device.as_str())
}

/// What tells Home Assistant to unload a per-entity config but keep its registry entry, so the same
/// `unique_id` can move into the device message with its name, area and settings intact.
pub const MIGRATE_PAYLOAD: &[u8] = br#"{"migrate_discovery":true}"#;

/// Whether a message on [`legacy_config_filter`] is a per-entity config Kessel published for
/// `device`, and so one to migrate and clear.
///
/// Matched on the device identifier inside the payload, not on the topic alone: another
/// integration may use the same node id. The migration marker and empty clears are not configs.
#[must_use]
pub fn is_legacy_config(device: &DeviceId, topic: &str, payload: &[u8]) -> bool {
    let mut levels = topic.split('/');
    let shaped = levels.next() == Some("homeassistant")
        && levels.next().is_some_and(|component| component != "device")
        && levels.next() == Some(device.as_str())
        && levels.next().is_some()
        && levels.next() == Some("config")
        && levels.next().is_none();
    if !shaped || payload.is_empty() {
        return false;
    }
    let identifier = format!("{DEVICE_ID_PREFIX}{}", device.as_str());
    serde_json::from_slice::<Value>(payload)
        .ok()
        .and_then(|config| {
            config
                .get("device")?
                .get("identifiers")?
                .as_array()
                .cloned()
        })
        .is_some_and(|identifiers| {
            identifiers
                .iter()
                .any(|id| id.as_str() == Some(identifier.as_str()))
        })
}

/// Whether this appliance runs programmes at all — it declares an `<activeProgram>` or a
/// `<selectedProgram>`.
///
/// Gated on the pointer, not on a non-empty programme list: an appliance can run a programme its
/// profile does not name.
fn runs_programmes(mapping: &DeviceMapping) -> bool {
    mapping.entries().iter().any(|entry| {
        matches!(
            entry.key.as_deref(),
            Some("BSH.Common.Root.ActiveProgram" | "BSH.Common.Root.SelectedProgram")
        )
    })
}

/// The device-relative `…/set` topics of the synthetic controls this appliance gets.
///
/// The subscription counterpart of the synthetic control configs, derived from the same predicate
/// so the two cannot disagree.
#[must_use]
pub fn synthetic_control_topics(mapping: &DeviceMapping) -> Vec<&'static str> {
    if runs_programmes(mapping) {
        vec![crate::bridge::STOP_PROGRAM_TOPIC]
    } else {
        Vec::new()
    }
}

/// The synthetic controls: configs that back no feature. Currently only "Stop programme".
///
/// There is no start button: the programme `select` already starts a programme on selection, and a
/// second path to starting an appliance (a cooktop included) is not wanted. Stop has no feature to
/// write to (`DELETE /ro/activeProgram`), so it is not a mapping entry.
fn synthetic_controls(
    base: &str,
    device: &DeviceId,
    mapping: &DeviceMapping,
) -> Vec<DiscoveryConfig> {
    if !runs_programmes(mapping) {
        return Vec::new();
    }
    let slug = device.as_str();
    let mut payload = Map::new();
    payload.insert("platform".to_owned(), Value::String("button".to_owned()));
    payload.insert(
        "unique_id".to_owned(),
        Value::String(format!("{slug}-_control.stop_program")),
    );
    payload.insert(
        "name".to_owned(),
        Value::String("Stop programme".to_owned()),
    );
    payload.insert(
        "default_entity_id".to_owned(),
        Value::String(format!(
            "button.{}",
            slugify(&format!("{slug}_stop_program"))
        )),
    );
    payload.insert(
        "icon".to_owned(),
        Value::String("mdi:stop-circle-outline".to_owned()),
    );
    payload.insert(
        "command_topic".to_owned(),
        Value::String(format!(
            "{base}/{slug}/{}",
            crate::bridge::STOP_PROGRAM_TOPIC
        )),
    );
    // `stop_program_command` ignores the payload.
    payload.insert(
        "payload_press".to_owned(),
        Value::String(BOOLEAN_ON.to_owned()),
    );
    // No `entity_category` or `enabled_by_default`: stop must always be visible.
    vec![DiscoveryConfig {
        id: "stop_program".to_owned(),
        component: "button",
        payload: Value::Object(payload),
    }]
}
