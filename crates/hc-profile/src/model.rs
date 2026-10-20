//! The entity model: what a `DeviceDescription` plus a `FeatureMapping` becomes.
//!
//! The shape serves the wire, not the XML. Every runtime update item is
//! `{"uid":…, "value":…, "access":…, "available":…, "min":…, "max":…, "stepSize":…,
//! "execution":…}`, so the model is keyed by `uid` and carries those fields. The
//! `FeatureMapping` supplies the dotted feature key.

use std::collections::BTreeMap;

/// Access rights as the description declares them.
///
/// The five known spellings are below; anything else is preserved verbatim so a feature is never
/// silently lost, and a write on an unknown access word is refused rather than guessed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Access {
    /// `access="none"`: neither readable nor writable right now.
    None,
    /// `access="read"`.
    Read,
    /// `access="readStatic"`: readable, and the value does not change.
    ReadStatic,
    /// `access="writeOnly"`.
    WriteOnly,
    /// `access="readWrite"`.
    ReadWrite,
    /// An access word this crate does not know.
    Other(String),
}

impl Access {
    /// Parses an access word. Case-insensitive: the profile XML writes `readWrite`, the appliance's
    /// description changes write `READWRITE`.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw.to_ascii_lowercase().as_str() {
            "none" => Self::None,
            "read" => Self::Read,
            "readstatic" => Self::ReadStatic,
            "writeonly" => Self::WriteOnly,
            "readwrite" => Self::ReadWrite,
            _ => Self::Other(raw.to_owned()),
        }
    }

    /// Whether a value may be read.
    #[must_use]
    pub const fn is_readable(&self) -> bool {
        matches!(self, Self::Read | Self::ReadStatic | Self::ReadWrite)
    }

    /// Whether a value may be written.
    #[must_use]
    pub const fn is_writable(&self) -> bool {
        matches!(self, Self::WriteOnly | Self::ReadWrite)
    }
}

/// Which XML element an entity came from.
///
/// The `*List` and program-slot variants are entities in their own right: the `FeatureMapping`
/// gives them dotted keys (`BSH.Common.Root.StatusList`, `BSH.Common.Root.SelectedProgram`),
/// they carry `uid`/`access`/`available`, and the appliance addresses them by uid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum EntityKind {
    /// `<status>`.
    Status,
    /// `<setting>`.
    Setting,
    /// `<event>`.
    Event,
    /// `<command>`.
    Command,
    /// `<option>` declared at device level (carries its own `uid`).
    Option,
    /// `<program>`.
    Program,
    /// `<statusList>`.
    StatusList,
    /// `<settingList>`.
    SettingList,
    /// `<eventList>`.
    EventList,
    /// `<commandList>`.
    CommandList,
    /// `<optionList>`.
    OptionList,
    /// `<programGroup>`.
    ProgramGroup,
    /// `<activeProgram>`.
    ActiveProgram,
    /// `<selectedProgram>`.
    SelectedProgram,
    /// `<protectionPort>`.
    ProtectionPort,
}

impl EntityKind {
    /// Maps a `DeviceDescription` element's local name to a kind, if it names an entity.
    #[must_use]
    pub const fn from_element(local_name: &[u8]) -> Option<Self> {
        // A `match` on a byte slice keeps this a single const lookup with no allocation.
        Some(match local_name {
            b"status" => Self::Status,
            b"setting" => Self::Setting,
            b"event" => Self::Event,
            b"command" => Self::Command,
            b"option" => Self::Option,
            b"program" => Self::Program,
            b"statusList" => Self::StatusList,
            b"settingList" => Self::SettingList,
            b"eventList" => Self::EventList,
            b"commandList" => Self::CommandList,
            b"optionList" => Self::OptionList,
            b"programGroup" => Self::ProgramGroup,
            b"activeProgram" => Self::ActiveProgram,
            b"selectedProgram" => Self::SelectedProgram,
            b"protectionPort" => Self::ProtectionPort,
            _ => return None,
        })
    }

    /// Whether this kind groups other entities (and therefore becomes a parent on the stack).
    #[must_use]
    pub const fn is_container(self) -> bool {
        matches!(
            self,
            Self::StatusList
                | Self::SettingList
                | Self::EventList
                | Self::CommandList
                | Self::OptionList
                | Self::ProgramGroup
        )
    }
}

/// An inclusive numeric range with a step, straight off `min`/`max`/`stepSize`.
///
/// All three are integers in practice; floats appear only in `initValue`/`default`, which stay
/// [`String`] for that reason.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NumericRange {
    /// `min`, if declared.
    pub min: Option<i64>,
    /// `max`, if declared.
    pub max: Option<i64>,
    /// `stepSize`, if declared.
    pub step: Option<i64>,
}

impl NumericRange {
    /// Whether the description declared any of the three.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.min.is_none() && self.max.is_none() && self.step.is_none()
    }
}

/// What kind of value an entity carries.
///
/// Derived only from structure in the description: an `enumerationType` reference or a declared
/// numeric range.
///
/// `Numeric` means "the description declared a `min`/`max`/`stepSize`", not "the value is a
/// number": a `refCID=05` string or `refCID=A8` object can carry a range too. Ask
/// [`Entity::protocol_type`] for the actual type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ValueType {
    /// The value is a member of the enumeration with this `enid`.
    Enumeration {
        /// `enumerationType`; look it up with [`EntityModel::enum_type`].
        enid: u16,
    },
    /// The value is a number constrained by this range.
    Numeric(NumericRange),
    /// The description declares neither an enumeration nor a range.
    Unconstrained,
}

/// One addressable feature of the appliance.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Entity {
    /// The `uid`, as the wire uses it.
    pub uid: u16,
    /// The dotted feature key from the `FeatureMapping`, e.g. `BSH.Common.Status.DoorState`.
    ///
    /// `None` when the mapping has no `<feature refUID=…>` for this uid; an unnamed entity is
    /// still an entity.
    pub key: Option<String>,
    /// Which element it came from.
    pub kind: EntityKind,
    /// `access`, if declared.
    pub access: Option<Access>,
    /// `available`, if declared.
    pub available: Option<bool>,
    /// Enumeration reference or numeric range, per [`ValueType`].
    pub value_type: ValueType,
    /// `refCID`, verbatim (two hex digits).
    pub ref_cid: Option<String>,
    /// `refDID`, verbatim (two-to-four hex digits).
    pub ref_did: Option<String>,
    /// `initValue`, verbatim: an integer, a float, `true`/`false` or a JSON object.
    pub init_value: Option<String>,
    /// `notifyOnChange`, if declared.
    pub notify_on_change: Option<bool>,
    /// `liveUpdate`, if declared.
    pub live_update: Option<bool>,
    /// The `uid` of the enclosing list, if any.
    pub parent: Option<u16>,
}

impl Entity {
    /// The coarse wire type this entity's `refCID` denotes, or [`None`] when it declares none this
    /// crate can decode.
    #[must_use]
    pub fn protocol_type(&self) -> Option<ProtocolType> {
        ProtocolType::of_ref_cid(self.ref_cid.as_deref())
    }

    /// The fine content type this entity's `refCID` denotes: what the value means, as opposed to
    /// how it casts. [`None`] when the code is absent, unparsable or a gap in the table.
    ///
    /// See [`ContentType`]; the two decodes read the same attribute and answer different questions.
    #[must_use]
    pub fn content_type(&self) -> Option<ContentType> {
        ContentType::of_ref_cid(self.ref_cid.as_deref())
    }
}

/// The fine-grained meaning a `refCID` denotes: what the value is, not how it casts.
///
/// [`ProtocolType`] answers "number, string or object?"; `ContentType` answers "a number of
/// what?", which is where a unit and a `device_class` come from. The two are independent:
/// `refCID=07` and `refCID=21` are both [`ProtocolType::Float`] but a temperature and a length.
///
/// The code table comes from third-party documentation, not a Home Connect specification. Only
/// codes with a consumer are named; every other known code is [`Self::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ContentType {
    /// `1` — a boolean.
    Boolean,
    /// `2` — a whole number with no further meaning.
    Integer,
    /// `3` — an enumeration; the element carries an `enumerationType` and the ordinal resolves
    /// through it.
    Enumeration,
    /// `4` — a fractional number with no further meaning.
    Float,
    /// `5` — text. A declared `min`/`max` on one of these bounds its **length**.
    String,
    /// `6` — a date and time.
    DateTime,
    /// `7` — degrees Celsius.
    TemperatureCelsius,
    /// `8` — degrees Fahrenheit.
    TemperatureFahrenheit,
    /// `16` — a duration, in seconds.
    TimeSpan,
    /// `17` — a percentage.
    Percent,
    /// `18` — a signal strength, in dBm.
    Dbm,
    /// `19` — a weight, in grams.
    Weight,
    /// `31` — a rotation rate, in rpm.
    Rpm,
    /// `33` — a length.
    Length,
    /// `35` — power, in watts.
    Power,
    /// `36` — energy, in watt-hours.
    Energy,
    /// A code the table names but this repository has no consumer for.
    ///
    /// Distinct from [`None`]: `Other` is a known code with no unit here, while `None` means the
    /// value's type is unknown.
    Other,
}

impl ContentType {
    /// Decodes a verbatim hex `refCID`, or [`None`] when it is absent, unparsable or a gap.
    ///
    /// The gaps (`0`, `9`, `11..=15`, `28`, `29`, `60`, `66..=128`) are [`None`]: a guessed unit is
    /// worse than none, because Home Assistant records and graphs whatever it is given.
    #[must_use]
    pub fn of_ref_cid(ref_cid: Option<&str>) -> Option<Self> {
        let code = u16::from_str_radix(ref_cid?.trim(), 16).ok()?;
        Some(match code {
            1 => Self::Boolean,
            2 => Self::Integer,
            3 => Self::Enumeration,
            4 => Self::Float,
            5 => Self::String,
            6 => Self::DateTime,
            7 => Self::TemperatureCelsius,
            8 => Self::TemperatureFahrenheit,
            16 => Self::TimeSpan,
            17 => Self::Percent,
            18 => Self::Dbm,
            19 => Self::Weight,
            31 => Self::Rpm,
            33 => Self::Length,
            35 => Self::Power,
            36 => Self::Energy,
            // Known but unnamed. The ranges mirror `ProtocolType::of_ref_cid`'s, so both decodes
            // have the same gaps.
            10 | 20..=27 | 30 | 32 | 34 | 37..=59 | 61..=65 | 129..=194 => Self::Other,
            _ => return None,
        })
    }
}

/// The coarse wire type a `refCID` denotes: how a reported value casts.
///
/// `refCID` is the appliance's data-type code, carried verbatim on [`Entity::ref_cid`]. The code
/// table comes from third-party documentation, not a Home Connect specification. An unknown or
/// absent code is [`None`], never a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ProtocolType {
    /// `true` / `false`.
    Boolean,
    /// A whole number. An enumeration member also arrives as one (content type 3).
    Integer,
    /// A fractional number.
    Float,
    /// Text. A declared `min`/`max` on one of these bounds its **length**, not its value.
    String,
    /// A structured value — a JSON object or list, e.g. `{"length":0,"list":[]}`.
    Object,
}

impl ProtocolType {
    /// Decodes a verbatim two-to-four-digit hex `refCID`, or [`None`] when it is absent, unparsable
    /// or not a code the table names.
    ///
    /// The table has gaps (9, 11–15, 28, 29, 60, 66–128), and a caller must be able to tell "this
    /// is text" from "unknown"; a guessed type can put a JSON object on a Home Assistant `number`.
    #[must_use]
    pub fn of_ref_cid(ref_cid: Option<&str>) -> Option<Self> {
        let code = u16::from_str_radix(ref_cid?.trim(), 16).ok()?;
        Some(match code {
            1 => Self::Boolean,
            2 | 3 | 16 | 18..=21 | 24 | 31 | 32 | 37 | 38 | 47 | 58 | 59 => Self::Integer,
            4 | 7 | 8 | 17 | 33..=36 | 39 | 41 | 43..=46 | 51..=57 | 64 | 65 => Self::Float,
            5 | 6 | 10 | 22 | 23 | 30 | 48 | 50 | 61 => Self::String,
            25..=27 | 40 | 42 | 49 | 62 | 63 | 129..=194 => Self::Object,
            _ => return None,
        })
    }

    /// Whether a value of this type is a number an operator can be asked to type into a box.
    #[must_use]
    pub const fn is_numeric(self) -> bool {
        matches!(self, Self::Integer | Self::Float)
    }
}

/// An `<option refUID=…>` inside a `<program>`.
///
/// Not an [`Entity`]: it references an existing uid and overlays program-specific access,
/// availability and bounds on it.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ProgramOption {
    /// The `refUID` this option constrains.
    pub ref_uid: u16,
    /// `access` for this option within this program.
    pub access: Option<Access>,
    /// `available` for this option within this program.
    pub available: Option<bool>,
    /// `liveUpdate`, if declared.
    pub live_update: Option<bool>,
    /// Program-specific bounds, if declared.
    pub range: NumericRange,
    /// `default`, verbatim.
    pub default: Option<String>,
    /// `initValue`, verbatim.
    pub init_value: Option<String>,
    /// `enumerationType`, if this option is an enum within this program.
    pub enid: Option<u16>,
}

/// A selectable program.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Program {
    /// The program `uid`.
    pub uid: u16,
    /// The dotted key, e.g. `Dishcare.Dishwasher.Program.Eco50`.
    pub key: Option<String>,
    /// `available`, if declared.
    pub available: Option<bool>,
    /// `execution` (`none` / `selectOnly` / `selectAndStart`), verbatim.
    pub execution: Option<String>,
    /// The `uid` of the enclosing `<programGroup>`.
    pub group: Option<u16>,
    /// The options this program accepts, in declaration order.
    pub options: Vec<ProgramOption>,
}

/// An enumeration: the values the description allows, and the names the mapping gives them.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct EnumType {
    /// `enid`.
    pub enid: u16,
    /// The dotted key from `<enumDescription enumKey=…>`, e.g. `BSH.Common.EnumType.DoorState`.
    pub key: Option<String>,
    /// Value → name, from the `FeatureMapping`'s `<enumMember refValue=…>`.
    pub members: BTreeMap<i64, String>,
    /// The values the `DeviceDescription` declares for this appliance, in declaration order.
    ///
    /// Kept separate from [`Self::members`]: the mapping names the whole enumeration, while the
    /// description narrows it to what this appliance actually supports.
    pub declared_values: Vec<i64>,
}

impl EnumType {
    /// The name of a value, if the mapping supplies one.
    #[must_use]
    pub fn member(&self, value: i64) -> Option<&str> {
        self.members.get(&value).map(String::as_str)
    }
}

/// The `<description>` block: what appliance this is, per the description file itself.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct DeviceMeta {
    /// `<type>`, e.g. `Dishwasher`.
    pub device_type: Option<String>,
    /// `<brand>`, e.g. `BOSCH`.
    pub brand: Option<String>,
    /// `<model>`: the VIB.
    pub model: Option<String>,
    /// `<version>`: DDF version.
    pub version: Option<String>,
    /// `<revision>`: DDF revision.
    pub revision: Option<String>,
}

/// An entity the parser refused, with the reason.
///
/// Recorded rather than raised: one malformed entity must never take down a device.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SkippedEntity {
    /// The element's local name, e.g. `status`.
    pub element: String,
    /// The `uid`/`refUID` attribute as written, if it was present at all.
    pub raw_uid: Option<String>,
    /// Why it was skipped.
    pub reason: SkipReason,
}

/// Why an entity was skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkipReason {
    /// The element carried no `uid`/`refUID` at all, so nothing can address it.
    MissingUid,
    /// The `uid` was present but not a 16-bit hexadecimal number.
    UnparsableUid,
    /// The element's text was not valid UTF-8.
    ///
    /// Only for element bodies. An undecodable attribute is treated as absent, so an
    /// undecodable `uid` surfaces as [`Self::MissingUid`].
    NotUtf8,
}

impl core::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match *self {
            Self::MissingUid => "no uid attribute",
            Self::UnparsableUid => "uid is not a 16-bit hex number",
            Self::NotUtf8 => "element text is not valid UTF-8",
        })
    }
}

/// The parsed appliance capability set.
#[derive(Debug, Clone, Default)]
pub struct EntityModel {
    meta: DeviceMeta,
    entities: BTreeMap<u16, Entity>,
    enums: BTreeMap<u16, EnumType>,
    programs: BTreeMap<u16, Program>,
}

impl EntityModel {
    /// Builds a model from already-parsed parts.
    #[must_use]
    pub(crate) const fn new(
        meta: DeviceMeta,
        entities: BTreeMap<u16, Entity>,
        enums: BTreeMap<u16, EnumType>,
        programs: BTreeMap<u16, Program>,
    ) -> Self {
        Self {
            meta,
            entities,
            enums,
            programs,
        }
    }

    /// What the description says this appliance is.
    #[must_use]
    pub const fn meta(&self) -> &DeviceMeta {
        &self.meta
    }

    /// Every entity, ordered by uid.
    #[must_use]
    pub const fn entities(&self) -> &BTreeMap<u16, Entity> {
        &self.entities
    }

    /// One entity by uid.
    #[must_use]
    pub fn entity(&self, uid: u16) -> Option<&Entity> {
        self.entities.get(&uid)
    }

    /// One entity by its dotted feature key.
    ///
    /// Linear scan; not a hot path.
    #[must_use]
    pub fn entity_by_key(&self, key: &str) -> Option<&Entity> {
        self.entities
            .values()
            .find(|entity| entity.key.as_deref() == Some(key))
    }

    /// Every enumeration, ordered by enid.
    #[must_use]
    pub const fn enum_types(&self) -> &BTreeMap<u16, EnumType> {
        &self.enums
    }

    /// One enumeration by enid.
    #[must_use]
    pub fn enum_type(&self, enid: u16) -> Option<&EnumType> {
        self.enums.get(&enid)
    }

    /// The enumeration an entity's value is drawn from, if it has one.
    #[must_use]
    pub fn enum_type_of(&self, entity: &Entity) -> Option<&EnumType> {
        match entity.value_type {
            ValueType::Enumeration { enid } => self.enum_type(enid),
            ValueType::Numeric(_) | ValueType::Unconstrained => None,
        }
    }

    /// Every program, ordered by uid.
    ///
    /// Empty for an appliance with no programs (a fridge, for example).
    #[must_use]
    pub const fn programs(&self) -> &BTreeMap<u16, Program> {
        &self.programs
    }

    /// One program by uid.
    #[must_use]
    pub fn program(&self, uid: u16) -> Option<&Program> {
        self.programs.get(&uid)
    }

    /// One program by its dotted key.
    #[must_use]
    pub fn program_by_key(&self, key: &str) -> Option<&Program> {
        self.programs
            .values()
            .find(|program| program.key.as_deref() == Some(key))
    }
}
