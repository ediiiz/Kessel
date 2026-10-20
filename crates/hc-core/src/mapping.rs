//! The generic feature → MQTT topic / Home Assistant entity mapping.
//!
//! This module is pure: it turns an [`hc_profile::EntityModel`] into device-relative topic paths
//! and the per-entity data a discovery config needs; [`crate::bridge`] publishes it.
//!
//! Coverage is total, with no allowlist: every feature whose node kind carries a value gets a state
//! topic, every writable one also gets a `…/set` topic, and the only way to withhold a feature is an
//! explicit [`Excludes`] entry. The read side is keyed off the node kind, not `access`: the fridge
//! declares `access="none"` on settings it reports values for, and `readWrite` on containers that
//! report nothing.
//!
//! The topic path is the dotted feature key with `.` replaced by `/` ([`feature_path`]); the bridge
//! prepends `<base>/<device>/`. The device component of every `unique_id` and topic is a
//! caller-supplied [`DeviceId`] slug; this module never sees the `haId`, which on the AES path
//! contains the appliance's MAC address.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use hc_profile::{Access, Entity, EntityKind, EntityModel, EnumType, NumericRange, ValueType};

/// A stable, operator-assigned device slug, used in topics and `unique_id`s.
///
/// It is not the `haId`, serial or MAC, so `Debug`/`Display` render it verbatim.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(String);

impl DeviceId {
    /// Wraps a caller-assigned device slug. The caller ensures it is not an appliance identifier;
    /// no format is enforced.
    #[must_use]
    pub fn new(slug: impl Into<String>) -> Self {
        Self(slug.into())
    }

    /// The slug, borrowed.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The Home Assistant platform an entity is surfaced as.
///
/// Derived from the model's kind, access and value type. Topic coverage does not depend on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EntityClass {
    /// A writable Setting/Option assumed to be boolean.
    Switch,
    /// A writable enumeration.
    Select,
    /// A readable value with no more specific class; the generic fallback.
    Sensor,
    /// An event, or a door-style two-member on/off enumeration.
    BinarySensor,
    /// A writable numeric, with `min`/`max`/`step` taken from the entity.
    Number,
    /// A `<command>`.
    Button,
}

/// A Home Assistant `device_class`.
///
/// [`Self::Enum`] is structural (an enumeration-typed sensor); the rest come from
/// `hc_profile::ContentType`. There is deliberately no `battery` or percentage class: HA has no
/// percentage class, and `battery` would be wrong on, say, an oven's programme progress, so a
/// percentage carries a unit and no class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeviceClass {
    /// An enumeration sensor.
    Enum,
    /// A temperature, in whichever scale [`EntityMapping::unit`] names.
    Temperature,
    /// A span of time.
    Duration,
    /// A radio signal strength.
    SignalStrength,
    /// A mass.
    Weight,
    /// Power.
    Power,
    /// Energy.
    Energy,
}

impl DeviceClass {
    /// The token Home Assistant reads.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enum => "enum",
            Self::Temperature => "temperature",
            Self::Duration => "duration",
            Self::SignalStrength => "signal_strength",
            Self::Weight => "weight",
            Self::Power => "power",
            Self::Energy => "energy",
        }
    }
}

/// Which section of Home Assistant's device page an entity is filed under.
///
/// Uncategorised entities appear in the prominent Controls/Sensors area. An appliance has up to
/// ~200 features and a handful that matter daily, so the rest are filed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EntityCategory {
    /// A control the operator may want but not daily — a writable setting or a command.
    Config,
    /// Information about the appliance rather than a control — a read-only status.
    Diagnostic,
}

impl EntityCategory {
    /// The token Home Assistant reads.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Diagnostic => "diagnostic",
        }
    }
}

/// A Home Assistant `state_class`: whether HA records long-term statistics, and of what kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StateClass {
    /// A value that moves in both directions — a temperature, a percentage, a signal strength.
    Measurement,
    /// A counter that only rises. `Measurement` on a counter would make HA report a meaningless
    /// mean and median.
    TotalIncreasing,
}

impl StateClass {
    /// The token Home Assistant reads.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Measurement => "measurement",
            Self::TotalIncreasing => "total_increasing",
        }
    }
}

/// Everything the bridge needs to publish, subscribe to and describe one feature.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EntityMapping {
    /// The wire `uid`.
    pub uid: u16,
    /// The dotted feature key, if the `FeatureMapping` named this uid.
    pub key: Option<String>,
    /// Which `DeviceDescription` element this came from, so consumers can tell a value-bearing
    /// node from a container without consulting the model.
    pub kind: EntityKind,
    /// The device-relative state topic (`…/state`), present iff the node kind carries a value,
    /// regardless of its `access` word.
    pub state_topic: Option<String>,
    /// The device-relative command topic (`…/set`), present iff the feature is writable.
    pub command_topic: Option<String>,
    /// `"<device_id>-<feature_key>"` (uid fallback when unnamed). Stable, collision-free per device.
    pub unique_id: String,
    /// The HA platform hint.
    pub class: EntityClass,
    /// The HA `device_class`, when the model determines one.
    pub device_class: Option<DeviceClass>,
    /// The HA `unit_of_measurement`, when the content type names one. Always a literal from the
    /// content-type table, never a value read off the appliance.
    pub unit: Option<&'static str>,
    /// The HA `state_class`, for a numeric sensor HA should keep statistics for.
    pub state_class: Option<StateClass>,
    /// Which section of HA's device page this belongs in; [`None`] is the prominent one.
    pub entity_category: Option<EntityCategory>,
    /// Whether HA should enable this entity on discovery. `false` only hides it: the config and
    /// state topic are still published.
    pub enabled_by_default: bool,
    /// The numeric bounds for a [`EntityClass::Number`], taken from the entity.
    ///
    /// [`None`] on a `Number` means the appliance stated no usable bounds, not that there is no
    /// control; [`crate::discovery`] decides what to publish then.
    pub number_range: Option<NumericRange>,
    /// Enum member names, for a [`EntityClass::Select`] or an enum [`EntityClass::Sensor`].
    pub options: Vec<String>,
}

/// The set of feature keys to withhold; the only way a feature leaves the mapping.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Excludes {
    keys: BTreeSet<String>,
}

impl Excludes {
    /// No exclusions — the total mapping.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Excludes each of the given feature keys.
    #[must_use]
    pub fn of<I, S>(keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            keys: keys.into_iter().map(Into::into).collect(),
        }
    }

    /// Whether this feature key is excluded.
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.keys.contains(key)
    }
}

/// A selectable program, with its name normalised for use as a select option and write value.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProgramEntry {
    /// The program `uid`.
    pub uid: u16,
    /// The dotted program key, verbatim (e.g. `Dishcare.Dishwasher.Program.Eco50`).
    pub key: Option<String>,
    /// The normalised name (lower-cased, `.`→`_`), when the program is named.
    pub normalized_name: Option<String>,
    /// `execution` (`none` / `selectOnly` / `selectAndStart`), verbatim; `start` is gated on it.
    pub execution: Option<String>,
}

/// The generic mapping for one device: one [`EntityMapping`] per non-excluded entity, plus the
/// device's selectable programs with normalised names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceMapping {
    entries: Vec<EntityMapping>,
    programs: Vec<ProgramEntry>,
}

impl DeviceMapping {
    /// Builds the total, generic mapping: one [`EntityMapping`] per entity in `model` whose
    /// feature key is not in `excludes`.
    #[must_use]
    pub fn build(device_id: &DeviceId, model: &EntityModel, excludes: &Excludes) -> Self {
        Self::build_with(device_id, model, excludes, &LiveDescriptions::default())
    }

    /// [`Self::build`], with the appliance's live description changes overlaid on the archive's
    /// snapshot. Where `live` has an answer it wins. An empty overlay is exactly [`Self::build`].
    #[must_use]
    pub fn build_with(
        device_id: &DeviceId,
        model: &EntityModel,
        excludes: &Excludes,
        live: &LiveDescriptions,
    ) -> Self {
        let entries = model
            .entities()
            .values()
            .filter(|entity| !is_excluded(entity, excludes))
            .map(|entity| map_entity(device_id, model, &live.overlay(entity)))
            .collect();
        let programs = model
            .programs()
            .values()
            .map(|program| ProgramEntry {
                uid: program.uid,
                key: program.key.clone(),
                normalized_name: program.key.as_deref().map(normalize_program_name),
                execution: program.execution.clone(),
            })
            .collect();
        Self { entries, programs }
    }

    /// Every entity mapping, ordered by uid.
    #[must_use]
    pub fn entries(&self) -> &[EntityMapping] {
        &self.entries
    }

    /// The device's selectable programs, with normalised names.
    #[must_use]
    pub fn programs(&self) -> &[ProgramEntry] {
        &self.programs
    }

    /// Every state topic produced (one per value-bearing, non-excluded feature).
    pub fn state_topics(&self) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .filter_map(|entry| entry.state_topic.as_deref())
    }

    /// Every command topic produced (one per writable, non-excluded feature).
    pub fn command_topics(&self) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .filter_map(|entry| entry.command_topic.as_deref())
    }
}

/// Whether an entity is withheld by an explicit [`Excludes`] entry. A keyless entity cannot be
/// excluded and is always mapped.
fn is_excluded(entity: &Entity, excludes: &Excludes) -> bool {
    entity
        .key
        .as_deref()
        .is_some_and(|key| excludes.contains(key))
}

/// The device-relative topic path of a feature key: `.` replaced by `/`.
#[must_use]
pub fn feature_path(key: &str) -> String {
    key.replace('.', "/")
}

/// Normalises a program name: lower-cased, `.`→`_`.
#[must_use]
pub fn normalize_program_name(key: &str) -> String {
    key.to_lowercase().replace('.', "_")
}

// --- internals ------------------------------------------------------------------------------

/// Whether a declared range is usable as Home Assistant number bounds.
///
/// HA requires a positive `step`, and a range must be able to hold the reported value; the fridge,
/// for example, declares `min="0" max="0" stepSize="0"` on setpoints that report -18 °C. An
/// unusable range still yields a control: the entity stays a [`EntityClass::Number`] with no
/// bounds, and [`crate::discovery`] publishes a non-binding range, because whether a write is
/// accepted is the appliance's decision.
const fn is_settable(range: NumericRange) -> bool {
    let (Some(min), Some(max)) = (range.min, range.max) else {
        // No declared bounds at all. HA would invent `1..=100`; we decline to.
        return false;
    };
    if min >= max {
        return false;
    }
    // An absent step is HA's default of 1, which is fine. A declared non-positive one is not.
    !matches!(range.step, Some(step) if step <= 0)
}

/// Whether an entity's declared access permits reading.
fn is_readable(entity: &Entity) -> bool {
    entity.access.as_ref().is_some_and(Access::is_readable)
}

/// Whether this kind of node carries a value: the read side of coverage.
///
/// Keyed off the kind, not `access`, because the two disagree on real appliances: the fridge
/// declares `access="none"` on settings it reports values for, and `access="readWrite"` on
/// `settingList` containers that report nothing.
///
/// Containers (`*List`, `<programGroup>`), programme definitions, commands (invoked, never read)
/// and the hob's `<protectionPort>` carry no value. Written as a positive list so a new
/// [`hc_profile::EntityKind`] is excluded until someone decides it reports something. Shared with
/// [`crate::discovery`].
pub(crate) const fn carries_value(kind: EntityKind) -> bool {
    matches!(
        kind,
        EntityKind::Status
            | EntityKind::Setting
            | EntityKind::Event
            | EntityKind::Option
            | EntityKind::ActiveProgram
            | EntityKind::SelectedProgram
    )
}

/// Whether this kind's `options` are programme keys rather than enum members.
///
/// `<selectedProgram>`/`<activeProgram>` declare no enumeration; [`program_options`] fills their
/// options from the `<program>` list. [`crate::presentation`] uses this to name an option from the
/// right catalogue table.
pub(crate) const fn offers_programs(kind: EntityKind) -> bool {
    matches!(
        kind,
        EntityKind::SelectedProgram | EntityKind::ActiveProgram
    )
}

/// Whether an entity gets a `…/set` topic.
///
/// `readWrite`/`writeOnly` do. `access="none"` does too, on an addressable kind
/// ([`is_addressable`]): the appliance declines to say rather than refuses, so the write is
/// attempted and the appliance decides. `read`, `readStatic`, an absent attribute and an
/// unrecognised word are refused. The addressable gate keeps the hob's `<protectionPort>`
/// (`access="none"`) from becoming a button.
fn is_writable(entity: &Entity) -> bool {
    if matches!(entity.access, Some(Access::None)) {
        return is_addressable(entity.kind);
    }
    entity.access.as_ref().is_some_and(Access::is_writable)
}

/// Whether this kind of node can be an entity an operator sees or acts on at all.
///
/// [`carries_value`] plus `<command>`, which reports nothing but is invoked as a button.
pub(crate) const fn is_addressable(kind: EntityKind) -> bool {
    carries_value(kind) || matches!(kind, EntityKind::Command)
}

/// The relative-path key for an entity: its feature key, or `uid.<HEX>` for an unnamed entity so
/// that it still gets a stable topic.
fn relative_key(entity: &Entity) -> String {
    entity
        .key
        .clone()
        .unwrap_or_else(|| format!("uid.{:04X}", entity.uid))
}

/// Maps one entity to its [`EntityMapping`].
fn map_entity(device_id: &DeviceId, model: &EntityModel, entity: &Entity) -> EntityMapping {
    let relative_key = relative_key(entity);
    let path = feature_path(&relative_key);
    let state_topic = carries_value(entity.kind).then(|| format!("{path}/state"));
    let command_topic = is_writable(entity).then(|| format!("{path}/set"));
    // The device component is the caller's slug, never the haId.
    let unique_id = format!("{}-{relative_key}", device_id.as_str());
    let class = classify(model, entity);
    let (device_class, unit) = class_and_unit(class, entity);
    let state_class = state_class_of(class, entity);
    let options = if offers_programs(entity.kind) {
        program_options(model)
    } else {
        match entity.value_type {
            ValueType::Enumeration { .. } => enum_options(model, entity),
            _ => Vec::new(),
        }
    };
    // An unusable declared range becomes `None`, so discovery can tell "the appliance said 2..8"
    // from "the appliance said nothing usable".
    let number_range = match (class, entity.value_type) {
        (EntityClass::Number, ValueType::Numeric(range)) if is_settable(range) => Some(range),
        _ => None,
    };
    EntityMapping {
        uid: entity.uid,
        key: entity.key.clone(),
        kind: entity.kind,
        state_topic,
        command_topic,
        unique_id,
        class,
        device_class,
        unit,
        state_class,
        entity_category: entity_category_of(entity, class),
        enabled_by_default: is_primary(entity),
        number_range,
        options,
    }
}

/// The appliance's declared programmes, in declaration order: the options of a programme pointer.
///
/// A `<selectedProgram>`/`<activeProgram>` value is the uid of a `<program>`, and these are the
/// strings [`crate::bridge::rendered_value`] resolves it to, so a published state always matches
/// an option. Unnamed programmes are omitted. An empty list is valid (the fridge has no
/// programmes).
fn program_options(model: &EntityModel) -> Vec<String> {
    model
        .programs()
        .values()
        .filter_map(|program| program.key.clone())
        .collect()
}

/// Enum member names for an entity, in declared order (select options / enum-sensor states).
fn enum_options(model: &EntityModel, entity: &Entity) -> Vec<String> {
    model
        .enum_type_of(entity)
        .map(member_names)
        .unwrap_or_default()
}

/// The names of an enumeration's declared values, in declaration order.
fn member_names(enum_type: &EnumType) -> Vec<String> {
    enum_type
        .declared_values
        .iter()
        .filter_map(|value| enum_type.member(*value).map(str::to_owned))
        .collect()
}

/// Whether an enumeration's declared members are literally `Off`/`On`, the only shape a Home
/// Assistant `binary_sensor` can represent.
///
/// A two-member test is not enough: `Open`/`Closed`, `Left`/`Right` or `Level01`/`Level02` would
/// match neither `binary_sensor` payload and read `unknown` forever, and the two door states are
/// declared in opposite orders on different appliances, so position means nothing. Those become
/// enum sensors instead.
fn is_on_off_enumeration(model: &EntityModel, entity: &Entity) -> bool {
    let Some(enum_type) = model.enum_type_of(entity) else {
        return false;
    };
    if enum_type.declared_values.len() != 2 {
        return false;
    }
    let names: Vec<&str> = enum_type
        .declared_values
        .iter()
        .filter_map(|value| enum_type.member(*value))
        .collect();
    names.contains(&"Off") && names.contains(&"On")
}

/// Whether an unconstrained entity may be a boolean (classification rule 7).
///
/// The description has no boolean flag, but `refCID` can rule it out. An undecodable code keeps
/// the assumption; a refuted entity falls back to a read-only sensor.
fn is_maybe_boolean(entity: &Entity) -> bool {
    entity
        .protocol_type()
        .is_none_or(|protocol| protocol == hc_profile::ProtocolType::Boolean)
}

/// Whether a bounded entity's declared type really is a number.
///
/// Bounds alone are not enough: a `refCID=05` string carries length bounds and a `refCID=A8`
/// object carries bounds too. An undecodable `refCID` is treated as numeric.
fn is_really_numeric(entity: &Entity) -> bool {
    entity
        .protocol_type()
        .is_none_or(hc_profile::ProtocolType::is_numeric)
}

/// Assigns an [`EntityClass`] from the model's kind, access, value type and `refCID`; first match
/// wins:
///
/// 1. a command → [`Button`](EntityClass::Button);
/// 2. an event → [`BinarySensor`](EntityClass::BinarySensor);
/// 3. a writable `<selectedProgram>` or a writable enumeration → [`Select`](EntityClass::Select);
/// 4. a readable `Off`/`On` enumeration → [`BinarySensor`](EntityClass::BinarySensor);
/// 5. any other readable enumeration → [`Sensor`](EntityClass::Sensor) with `device_class` enum;
/// 6. a writable bounded value whose `refCID` is numeric → [`Number`](EntityClass::Number);
/// 7. a writable unconstrained Setting/Option not known to be non-boolean →
///    [`Switch`](EntityClass::Switch) (an assumption, since descriptions declare no boolean type);
/// 8. a read-only Setting/Status/Option whose `refCID` decodes to boolean →
///    [`BinarySensor`](EntityClass::BinarySensor). Stricter than rule 7 because it offers no
///    control, so an undecodable code stays a generic sensor;
/// 9. everything else → [`Sensor`](EntityClass::Sensor).
fn classify(model: &EntityModel, entity: &Entity) -> EntityClass {
    if matches!(entity.kind, EntityKind::Command | EntityKind::CommandList) {
        return EntityClass::Button;
    }
    if matches!(entity.kind, EntityKind::Event | EntityKind::EventList) {
        return EntityClass::BinarySensor;
    }
    // Rule 3, programme chooser: its options are programmes, not an enumeration. `<activeProgram>`
    // reports what is running and is not offered as a second control.
    if matches!(entity.kind, EntityKind::SelectedProgram) && is_writable(entity) {
        return EntityClass::Select;
    }
    match entity.value_type {
        ValueType::Enumeration { .. } if is_writable(entity) => EntityClass::Select,
        ValueType::Enumeration { .. } if is_readable(entity) => {
            if is_on_off_enumeration(model, entity) {
                EntityClass::BinarySensor
            } else {
                EntityClass::Sensor
            }
        }
        ValueType::Numeric(_) if is_writable(entity) && is_really_numeric(entity) => {
            EntityClass::Number
        }
        ValueType::Unconstrained
            if is_writable(entity)
                && matches!(entity.kind, EntityKind::Setting | EntityKind::Option)
                && is_maybe_boolean(entity) =>
        {
            EntityClass::Switch
        }
        // Rule 8.
        _ if !is_writable(entity)
            && matches!(
                entity.kind,
                EntityKind::Setting | EntityKind::Status | EntityKind::Option
            )
            && entity.protocol_type() == Some(hc_profile::ProtocolType::Boolean) =>
        {
            EntityClass::BinarySensor
        }
        // `ValueType` is `#[non_exhaustive]`; root, list and program entities also land here.
        _ => EntityClass::Sensor,
    }
}

/// Whether this entity's state is a member of a declared vocabulary rather than a measurement:
/// an enumeration ([`enum_options`]) or a programme pointer ([`program_options`]).
///
/// Both fill [`EntityMapping::options`], and HA rejects a sensor config with `options` unless it
/// has `device_class: enum`. A programme pointer's value is a uid, which decodes as a number, so
/// `value_type` alone does not reveal it.
const fn has_vocabulary(entity: &Entity) -> bool {
    matches!(entity.value_type, ValueType::Enumeration { .. }) || offers_programs(entity.kind)
}

/// The `device_class` and `unit_of_measurement` for a mapped entity.
///
/// A sensor with a vocabulary ([`has_vocabulary`]) is `enum` with no unit; otherwise both come
/// from the content type ([`content_class_and_unit`]). An unrecognised content type gets neither:
/// HA records and graphs whatever unit it is given, so a guess is worse than none.
fn class_and_unit(
    class: EntityClass,
    entity: &Entity,
) -> (Option<DeviceClass>, Option<&'static str>) {
    if has_vocabulary(entity) {
        // HA rejects `device_class: enum` outside `sensor` and discards the entity
        // (`discovery::sanitize_for_platform` also strips it).
        let device_class = matches!(class, EntityClass::Sensor).then_some(DeviceClass::Enum);
        return (device_class, None);
    }
    content_class_and_unit(entity.content_type())
}

/// The HA `device_class` and unit for a content type.
///
/// Units are the ones the content type names (`°C` for `temperatureCelsius`, `s` for a
/// `timeSpan`). `Percent` and `Rpm` get a unit and no class, because HA has none for them.
const fn content_class_and_unit(
    content: Option<hc_profile::ContentType>,
) -> (Option<DeviceClass>, Option<&'static str>) {
    use hc_profile::ContentType as Ct;
    let Some(content) = content else {
        return (None, None);
    };
    match content {
        Ct::TemperatureCelsius => (Some(DeviceClass::Temperature), Some("°C")),
        Ct::TemperatureFahrenheit => (Some(DeviceClass::Temperature), Some("°F")),
        Ct::TimeSpan => (Some(DeviceClass::Duration), Some("s")),
        Ct::Dbm => (Some(DeviceClass::SignalStrength), Some("dBm")),
        Ct::Weight => (Some(DeviceClass::Weight), Some("g")),
        Ct::Power => (Some(DeviceClass::Power), Some("W")),
        Ct::Energy => (Some(DeviceClass::Energy), Some("Wh")),
        // A unit with no class: HA names none of these.
        Ct::Percent => (None, Some("%")),
        Ct::Rpm => (None, Some("rpm")),
        _ => (None, None),
    }
}

/// The `state_class` for a mapped entity.
///
/// Only a numeric sensor gets one (HA refuses it on a `number` or `select`), decided by the content
/// type rather than by declared bounds. A key containing `Count` is treated as a lifetime counter
/// ([`StateClass::TotalIncreasing`]); this is a name heuristic.
fn state_class_of(class: EntityClass, entity: &Entity) -> Option<StateClass> {
    // A programme pointer's value is a uid, which is a label, not a quantity.
    if !matches!(class, EntityClass::Sensor) || has_vocabulary(entity) {
        return None;
    }
    if !entity.content_type().is_some_and(is_measurable) {
        return None;
    }
    let counts = entity
        .key
        .as_deref()
        .is_some_and(|key| key.contains("Count"));
    Some(if counts {
        StateClass::TotalIncreasing
    } else {
        StateClass::Measurement
    })
}

/// The leaf names of the features an operator looks at daily: these stay prominent and enabled,
/// everything else is filed and disabled by default (still published, one click from enabled).
///
/// The one presentation judgement here that is not derived from the appliance. Matched on the
/// leaf, so it works across models and cavity-scoped keys.
const PRIMARY_LEAVES: [&str; 15] = [
    "PowerState",
    "OperationState",
    "DoorState",
    "RemainingProgramTime",
    "ProgramProgress",
    "StartInRelative",
    "ActiveProgram",
    "SelectedProgram",
    "RemoteControlActive",
    "RemoteControlStartAllowed",
    "BackendConnected",
    "ProgramFinished",
    "ProgramAborted",
    "BatteryLevel",
    "ChargingState",
];

/// Whether this entity is in the prominent, enabled-by-default set ([`PRIMARY_LEAVES`]).
///
/// `<activeProgram>`/`<selectedProgram>` qualify by kind, whatever their key.
fn is_primary(entity: &Entity) -> bool {
    if matches!(
        entity.kind,
        EntityKind::ActiveProgram | EntityKind::SelectedProgram
    ) {
        return true;
    }
    entity.key.as_deref().is_some_and(|key| {
        let leaf = key.rsplit('.').next().unwrap_or(key);
        PRIMARY_LEAVES.contains(&leaf)
    })
}

/// Which section of HA's device page an entity is filed under.
///
/// A primary feature is uncategorised. Otherwise a Setting/Option that HA renders as a control,
/// or a command, is [`EntityCategory::Config`]; a Setting/Option HA renders as a reading, a
/// status or an event is [`EntityCategory::Diagnostic`]. This reads the published class rather than
/// `access`: a writable string such as a favourite's name is published as a sensor and is not
/// configurable from HA. Containers get no category.
fn entity_category_of(entity: &Entity, class: EntityClass) -> Option<EntityCategory> {
    if is_primary(entity) {
        return None;
    }
    let operable = matches!(
        class,
        EntityClass::Switch | EntityClass::Select | EntityClass::Number | EntityClass::Button
    );
    match entity.kind {
        EntityKind::Setting | EntityKind::Option => Some(if operable {
            EntityCategory::Config
        } else {
            EntityCategory::Diagnostic
        }),
        EntityKind::Command | EntityKind::CommandList => Some(EntityCategory::Config),
        EntityKind::Status | EntityKind::Event => Some(EntityCategory::Diagnostic),
        _ => None,
    }
}

/// Whether a content type is a number HA can keep statistics over.
const fn is_measurable(content: hc_profile::ContentType) -> bool {
    use hc_profile::ContentType as Ct;
    matches!(
        content,
        Ct::Integer
            | Ct::Float
            | Ct::TemperatureCelsius
            | Ct::TemperatureFahrenheit
            | Ct::TimeSpan
            | Ct::Percent
            | Ct::Dbm
            | Ct::Weight
            | Ct::Rpm
            | Ct::Length
            | Ct::Power
            | Ct::Energy
    )
}

/// What the appliance has said about its own features since the profile archive was taken.
///
/// The archive's `DeviceDescription.xml` is an onboarding snapshot; a running appliance revises
/// `access`, `available`, `min`, `max` and `stepSize` with `NOTIFY /ro/descriptionChange`.
/// Merging is sparse: a field an update does not mention keeps its previous value, since most
/// updates carry values only.
///
/// A feature that was ever writable keeps the shape of a control: appliances lock options while a
/// programme runs, and reclassifying them would delete the Home Assistant entity each time. The
/// appliance refuses a write to a locked control, and the bridge republishes the actual state.
///
/// Holds uids, access words and bounds only, so it is safe to `Debug`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveDescriptions {
    by_uid: BTreeMap<u16, hc_proto::DescriptionUpdate>,
    ever_writable: BTreeSet<u16>,
}

impl LiveDescriptions {
    /// Whether the appliance has revised anything yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_uid.is_empty()
    }

    /// Folds one update's description into the overlay and reports whether anything changed, so
    /// the caller re-announces discovery only when needed. A uid that does not fit a `u16` is
    /// ignored.
    pub fn absorb(&mut self, update: &hc_proto::ValueUpdate) -> bool {
        if !update.description.is_some() {
            return false;
        }
        let Ok(uid) = u16::try_from(update.uid) else {
            return false;
        };
        let entry = self.by_uid.entry(uid).or_default();
        let before = entry.clone();
        merge(entry, &update.description);
        let changed = *entry != before;
        if entry
            .access
            .as_deref()
            .is_some_and(|access| Access::parse(access).is_writable())
        {
            self.ever_writable.insert(uid);
        }
        changed
    }

    /// The entity as the appliance describes it now: the archive's, with any revision applied.
    /// Borrowed when nothing applies.
    ///
    /// Only `access`, `available` and numeric bounds are overlaid; an update item cannot revise the
    /// kind, key, `refCID` or enumeration. Bounds apply only to a [`ValueType::Numeric`].
    fn overlay<'e>(&self, entity: &'e Entity) -> Cow<'e, Entity> {
        let Some(live) = self.by_uid.get(&entity.uid) else {
            return Cow::Borrowed(entity);
        };
        let mut patched = entity.clone();
        if let Some(access) = live.access.as_deref() {
            let access = Access::parse(access);
            if access.is_writable()
                || !(is_writable(entity) || self.ever_writable.contains(&entity.uid))
            {
                patched.access = Some(access);
            } else if !is_writable(entity) {
                patched.access = Some(Access::ReadWrite);
            }
        }
        if let Some(available) = live.available {
            patched.available = Some(available);
        }
        if let ValueType::Numeric(range) = patched.value_type {
            patched.value_type = ValueType::Numeric(NumericRange {
                min: live.min.or(range.min),
                max: live.max.or(range.max),
                step: live.step.or(range.step),
            });
        }
        Cow::Owned(patched)
    }
}

/// Applies the fields `update` actually mentions, leaving the rest of `into` alone.
fn merge(into: &mut hc_proto::DescriptionUpdate, update: &hc_proto::DescriptionUpdate) {
    if update.access.is_some() {
        into.access.clone_from(&update.access);
    }
    if update.available.is_some() {
        into.available = update.available;
    }
    if update.min.is_some() {
        into.min = update.min;
    }
    if update.max.is_some() {
        into.max = update.max;
    }
    if update.step.is_some() {
        into.step = update.step;
    }
}
