//! The wire message model.
//!
//! Outbound ([`OutgoingMessage`]) and inbound ([`IncomingMessage`]) are different types. Outbound
//! serialisation is order-controlled, because the firmware parses JSON objects positionally: the
//! fields go out as `sID, msgID, resource, version, action`, then `data`, then `code`, and every
//! object inside `data` has a fixed key order. JSON is therefore built through typed structs and a
//! hand-driven map serializer, never through `serde_json::Value`, whose equality ignores key order.
//!
//! Inbound parsing is defensive: int-or-string coercion of `sID`/`msgID`/`version`, a `]"` → `]`
//! whole-frame repair, single-object-to-list `data` normalisation, and "a non-null `code` means
//! error" (not truthiness).

use core::fmt;

use serde::ser::{Serialize, SerializeMap as _, Serializer};
use serde_json::{Number, Value};

use crate::codes::ResponseCode;
use crate::error::ProtoError;

/// The wire verbs.
///
/// The handshake emits only `GET`, `RESPONSE` and `NOTIFY`; `POST` arrives inbound and is used for
/// writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Read a resource.
    Get,
    /// Device-initiated push (only `POST /ei/initialValues` in the handshake).
    Post,
    /// Answer to a request; reuses the request's `msgID`.
    Response,
    /// Fire-and-forget; no response awaited.
    Notify,
    /// Clear the active program; unused in the handshake.
    Delete,
}

impl Action {
    /// The exact wire token.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Response => "RESPONSE",
            Self::Notify => "NOTIFY",
            Self::Delete => "DELETE",
        }
    }

    /// Parses a wire token, or `None` if it is not one of the five verbs.
    #[must_use]
    pub fn from_wire(token: &str) -> Option<Self> {
        match token {
            "GET" => Some(Self::Get),
            "POST" => Some(Self::Post),
            "RESPONSE" => Some(Self::Response),
            "NOTIFY" => Some(Self::Notify),
            "DELETE" => Some(Self::Delete),
            _ => None,
        }
    }
}

impl Serialize for Action {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_wire())
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_wire())
    }
}

/// A `version` field, keeping its JSON type.
///
/// A RESPONSE echoes the received `version` verbatim, so a string version is re-emitted as a
/// string. For control flow use [`Version::as_int`] (`"4.2"` → `4`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Version {
    /// A JSON number.
    Int(i64),
    /// A JSON string, e.g. `"4.2"`.
    Str(String),
}

impl Version {
    /// The int-coerced value: a number as itself, a string parsed as an integer or truncated from
    /// a float. `None` if it is neither.
    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            Self::Str(text) => parse_int_lenient(text),
        }
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(number) => number.as_i64().map(Self::Int),
            Value::String(text) => Some(Self::Str(text.clone())),
            _ => None,
        }
    }
}

impl Serialize for Version {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Int(value) => serializer.serialize_i64(*value),
            Self::Str(text) => serializer.serialize_str(text),
        }
    }
}

/// The `deviceType` value in the `/ei/initialValues` RESPONSE.
///
/// The integer `2` on ei v1, the string `"Application"` otherwise. The JSON type matters (`2` is
/// not `"2"`), so this is an enum rather than a bare value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    /// ei v1: the integer `2`.
    IntTwo,
    /// ei ≠ 1: the string `"Application"`.
    Application,
}

impl DeviceType {
    /// `2` if the init message's ei version is `1`, else `"Application"`.
    #[must_use]
    pub const fn for_ei_version(ei_version: i64) -> Self {
        if ei_version == 1 {
            Self::IntTwo
        } else {
            Self::Application
        }
    }
}

impl Serialize for DeviceType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::IntTwo => serializer.serialize_i64(2),
            Self::Application => serializer.serialize_str("Application"),
        }
    }
}

/// The client's `deviceName`/`deviceID`.
///
/// Configuration, kept stable across reconnects; never generated per connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdentity {
    /// `deviceName`, e.g. `"hcpy"`.
    pub name: String,
    /// `deviceID`, e.g. `"0badcafe"`.
    pub id: String,
}

/// The `data` payload of an outbound message.
///
/// `data` is always a list on send. Handshake messages are data-less or carry one object; a
/// feature write carries one object per feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// No `data` key at all: omitted, not `null`.
    Empty,
    /// `[{"deviceType":…,"deviceName":…,"deviceID":…}]`, in that order.
    InitialValues {
        /// `deviceType`, emitted first.
        device_type: DeviceType,
        /// The configured `deviceName`/`deviceID`.
        identity: DeviceIdentity,
    },
    /// `[{"nonce":"<base64url>"}]` for `GET /ci/authentication`.
    Nonce(String),
    /// `[{"uid":…,"value":…},…]`: the feature-write body of a `POST /ro/values`.
    ///
    /// One item per feature written; key order inside each item matters (see [`WriteItem`]).
    ///
    /// Must not be empty: [`OutgoingMessage::values_write`] refuses an empty list, which would
    /// otherwise emit `"data":[]`. Building an [`OutgoingMessage`] by struct literal bypasses that
    /// check.
    Values(Vec<WriteItem>),
    /// `[{"program":<uid>,"options":[{"uid":…,"value":…},…]}]`: the program-start body of a
    /// `POST /ro/selectedProgram`.
    ///
    /// `program` is emitted before `options`. This frame can start a cooktop or an oven.
    Program {
        /// The program uid, emitted first.
        program: u32,
        /// The program options, each `{"uid":…,"value":…}` with `uid` first.
        ///
        /// May be empty, and then emits `"options":[]`: a program without options is ordinary,
        /// and the key stays present.
        options: Vec<WriteItem>,
    },
}

/// One `data` item of a feature write: `{"uid":<int>,"value":<scalar>}`, `uid` first.
///
/// The key order matters: appliance firmware requires `uid` to be the first attribute.
///
/// This type has no `Serialize` impl, so its field order does not control the wire order. The
/// private `WriteElem` below does; that is the declaration not to reorder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteItem {
    /// The target feature's `uid`.
    pub uid: u32,
    /// The value to write.
    pub value: WriteValue,
}

impl WriteItem {
    /// A write item, with the value converted from JSON.
    ///
    /// # Errors
    ///
    /// [`ProtoError::UnrepresentableWriteValue`] if `value` is not a JSON scalar; see
    /// [`WriteValue::from_json`].
    pub fn from_json(uid: u32, value: &Value) -> Result<Self, ProtoError> {
        Ok(Self {
            uid,
            value: WriteValue::from_json(value)?,
        })
    }
}

/// The `value` of a feature write: a JSON scalar, keeping its wire type.
///
/// Integers, non-integral numbers, strings and booleans are the known shapes; `null`, lists and
/// objects are not sent. The number is a [`Number`] rather than an `f64` because a `Number` cannot
/// hold `NaN` or an infinity, which would otherwise serialise silently as `"value":null`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteValue {
    /// A JSON boolean.
    Bool(bool),
    /// A JSON number, integral or not, type-preserved.
    Number(Number),
    /// A JSON string.
    Str(String),
}

impl WriteValue {
    /// Converts a `serde_json::Value` into a write value, or refuses it.
    ///
    /// # Errors
    ///
    /// [`ProtoError::UnrepresentableWriteValue`] for `null`, an array or an object, which have no
    /// known wire encoding as a feature `value`.
    pub fn from_json(value: &Value) -> Result<Self, ProtoError> {
        match value {
            Value::Bool(flag) => Ok(Self::Bool(*flag)),
            Value::Number(number) => Ok(Self::Number(number.clone())),
            Value::String(text) => Ok(Self::Str(text.clone())),
            // Default-deny: rendering these as JSON text (`"value":"null"`) would put an unknown
            // shape in front of a positional parser.
            Value::Null => Err(ProtoError::UnrepresentableWriteValue("null")),
            Value::Array(_) => Err(ProtoError::UnrepresentableWriteValue("array")),
            Value::Object(_) => Err(ProtoError::UnrepresentableWriteValue("object")),
        }
    }
}

impl Serialize for WriteValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Bool(flag) => serializer.serialize_bool(*flag),
            Self::Number(number) => number.serialize(serializer),
            Self::Str(text) => serializer.serialize_str(text),
        }
    }
}

impl Body {
    const fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }
}

/// A message Kessel sends. Serialises with a fixed key order; see the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingMessage {
    /// Session id, echoed from `/ei/initialValues`.
    pub s_id: u64,
    /// Correlation id.
    pub msg_id: u64,
    /// The resource path, e.g. `/ci/services`.
    pub resource: String,
    /// Service version, kept type-exact for the RESPONSE echo.
    pub version: Version,
    /// The verb.
    pub action: Action,
    /// The `data` payload.
    pub body: Body,
    /// A `code`; never set on an outbound handshake message, present for symmetry.
    pub code: Option<i64>,
}

impl OutgoingMessage {
    /// A data-less request or notify (`GET`/`NOTIFY` with no `data`).
    #[must_use]
    pub fn bare(
        s_id: u64,
        msg_id: u64,
        resource: impl Into<String>,
        version: Version,
        action: Action,
    ) -> Self {
        Self {
            s_id,
            msg_id,
            resource: resource.into(),
            version,
            action,
            body: Body::Empty,
            code: None,
        }
    }

    /// The `RESPONSE /ei/initialValues` carrying the client identity.
    #[must_use]
    pub fn initial_values_response(
        s_id: u64,
        msg_id: u64,
        version: Version,
        device_type: DeviceType,
        identity: &DeviceIdentity,
    ) -> Self {
        Self {
            s_id,
            msg_id,
            resource: "/ei/initialValues".to_owned(),
            version,
            action: Action::Response,
            body: Body::InitialValues {
                device_type,
                identity: identity.clone(),
            },
            code: None,
        }
    }

    /// A `POST /ro/selectedProgram` program start.
    ///
    /// Infallible: an empty `options` list is legal and emits `"options":[]`. This frame can start
    /// a cooktop or an oven.
    #[must_use]
    pub fn program_start(
        s_id: u64,
        msg_id: u64,
        version: Version,
        program: u32,
        options: Vec<WriteItem>,
    ) -> Self {
        Self {
            s_id,
            msg_id,
            resource: "/ro/selectedProgram".to_owned(),
            version,
            action: Action::Post,
            body: Body::Program { program, options },
            code: None,
        }
    }

    /// `GET /ci/authentication` with the nonce payload.
    #[must_use]
    pub fn authentication(
        s_id: u64,
        msg_id: u64,
        version: Version,
        nonce: impl Into<String>,
    ) -> Self {
        Self {
            s_id,
            msg_id,
            resource: "/ci/authentication".to_owned(),
            version,
            action: Action::Get,
            body: Body::Nonce(nonce.into()),
            code: None,
        }
    }

    /// A feature write: `POST /ro/values` with one `{"uid":…,"value":…}` item per feature.
    ///
    /// `version` is the caller's: it is the negotiated `ro` service version (see
    /// [`crate::Handshake::version_for`]). Building a frame here does not send it.
    ///
    /// # Errors
    ///
    /// [`ProtoError::EmptyWriteItemList`] if `items` is empty.
    pub fn values_write(
        s_id: u64,
        msg_id: u64,
        version: Version,
        items: Vec<WriteItem>,
    ) -> Result<Self, ProtoError> {
        // An empty list would serialise as `"data":[]`; refuse it instead.
        if items.is_empty() {
            return Err(ProtoError::EmptyWriteItemList);
        }
        Ok(Self {
            s_id,
            msg_id,
            resource: "/ro/values".to_owned(),
            version,
            action: Action::Post,
            body: Body::Values(items),
            code: None,
        })
    }

    /// Whether a RESPONSE is expected: true only for `GET`. `NOTIFY` and `RESPONSE` are
    /// fire-and-forget.
    #[must_use]
    pub const fn expects_response(&self) -> bool {
        matches!(self.action, Action::Get)
    }

    /// Serialises to the exact wire bytes: compact separators, ordered keys, `data` a list,
    /// `data`/`code` omitted when absent.
    ///
    /// # Errors
    ///
    /// [`ProtoError::Decode`] only if the underlying serializer fails, which for these
    /// string-keyed, finite-value types does not happen in practice.
    pub fn to_wire_bytes(&self) -> Result<Vec<u8>, ProtoError> {
        serde_json::to_vec(self).map_err(|error| ProtoError::Decode(error.to_string()))
    }
}

/// The `data` object of the `/ei/initialValues` RESPONSE. `serde` emits struct fields in
/// declaration order, which fixes the wire order `deviceType, deviceName, deviceID`.
#[derive(serde::Serialize)]
#[expect(
    clippy::struct_field_names,
    reason = "the field names mirror the wire keys deviceType/deviceName/deviceID"
)]
struct InitialValuesElem<'a> {
    #[serde(rename = "deviceType")]
    device_type: DeviceType,
    #[serde(rename = "deviceName")]
    device_name: &'a str,
    #[serde(rename = "deviceID")]
    device_id: &'a str,
}

#[derive(serde::Serialize)]
struct NonceElem<'a> {
    nonce: &'a str,
}

/// One `{"uid":…,"value":…}` write item. Declaration order is wire order.
#[derive(serde::Serialize)]
struct WriteElem<'a> {
    // `uid` first: the firmware depends on it. Do not reorder these fields.
    uid: u32,
    value: &'a WriteValue,
}

/// One `{"program":…,"options":[…]}` body object. Declaration order is wire order.
#[derive(serde::Serialize)]
struct ProgramElem<'a> {
    // `program` first. Do not reorder these fields.
    program: u32,
    options: Vec<WriteElem<'a>>,
}

/// Borrows each [`WriteItem`] as its wire element, so the uid-first rule has one implementation.
fn write_elems(items: &[WriteItem]) -> Vec<WriteElem<'_>> {
    items
        .iter()
        .map(|item| WriteElem {
            uid: item.uid,
            value: &item.value,
        })
        .collect()
}

impl Serialize for OutgoingMessage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut len = 5;
        if !self.body.is_empty() {
            len += 1;
        }
        if self.code.is_some() {
            len += 1;
        }
        let mut map = serializer.serialize_map(Some(len))?;
        map.serialize_entry("sID", &self.s_id)?;
        map.serialize_entry("msgID", &self.msg_id)?;
        map.serialize_entry("resource", &self.resource)?;
        map.serialize_entry("version", &self.version)?;
        map.serialize_entry("action", &self.action)?;
        match &self.body {
            Body::Empty => {}
            Body::InitialValues {
                device_type,
                identity,
            } => {
                map.serialize_entry(
                    "data",
                    &[InitialValuesElem {
                        device_type: *device_type,
                        device_name: &identity.name,
                        device_id: &identity.id,
                    }],
                )?;
            }
            Body::Nonce(nonce) => {
                map.serialize_entry("data", &[NonceElem { nonce }])?;
            }
            Body::Values(items) => {
                map.serialize_entry("data", &write_elems(items))?;
            }
            Body::Program { program, options } => {
                map.serialize_entry(
                    "data",
                    &[ProgramElem {
                        program: *program,
                        options: write_elems(options),
                    }],
                )?;
            }
        }
        if let Some(code) = self.code {
            map.serialize_entry("code", &code)?;
        }
        map.end()
    }
}

/// A message Kessel received, after defensive parsing.
///
/// `Debug` is hand-written: `data` can hold a `/ci/info` body with `deviceID`/`mac`/`serialNumber`,
/// which must never appear in a `{:?}`.
#[derive(Clone)]
pub struct IncomingMessage {
    /// Session id, int-coerced.
    pub s_id: Option<u64>,
    /// Correlation id, int-coerced.
    pub msg_id: Option<u64>,
    /// The resource path.
    pub resource: Option<String>,
    /// The version, kept verbatim; use [`Version::as_int`] for logic.
    pub version: Option<Version>,
    /// The verb.
    pub action: Action,
    /// The `data` list, normalised: a single object is wrapped, `null`/absent is `None`.
    pub data: Option<Vec<Value>>,
    /// A `code`; any non-null value means error.
    pub code: Option<i64>,
}

/// Renders [`IncomingMessage`]'s `data` in a `Debug` as a value count, never the values.
struct DataSummary<'a>(&'a Option<Vec<Value>>);

impl fmt::Debug for DataSummary<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            None => f.write_str("None"),
            Some(values) => write!(f, "<{} values>", values.len()),
        }
    }
}

impl fmt::Debug for IncomingMessage {
    /// Renders the routing metadata; `data` is rendered only as a count (`<N values>`). Code that
    /// needs the decoded body reads the public `data` field explicitly.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IncomingMessage")
            .field("s_id", &self.s_id)
            .field("msg_id", &self.msg_id)
            .field("resource", &self.resource)
            .field("version", &self.version)
            .field("action", &self.action)
            .field("data", &DataSummary(&self.data))
            .field("code", &self.code)
            .finish()
    }
}

impl IncomingMessage {
    /// The typed error a `code` denotes, or `None` when there is no `code`.
    ///
    /// The predicate is `code.is_some()`, not truthiness: `"code":0` is an error.
    #[must_use]
    pub fn response_error(&self) -> Option<(i64, ResponseCode)> {
        self.code.map(|code| (code, ResponseCode::from_code(code)))
    }

    /// The two-character service key of the resource, e.g. `/ci/services` → `ci`.
    #[must_use]
    pub fn service_key(&self) -> Option<&str> {
        self.resource.as_deref().and_then(service_key)
    }
}

/// The two characters after the leading `/`: `/ci/services` → `ci`.
#[must_use]
pub fn service_key(resource: &str) -> Option<&str> {
    resource.strip_prefix('/').and_then(|rest| rest.get(..2))
}

/// Parses a frame, applying the `]"` → `]` whole-frame repair once on failure.
///
/// # Errors
///
/// [`ProtoError::Decode`] if the bytes are not JSON even after the repair, or
/// [`ProtoError::Malformed`] if the JSON is not an object or its `action` is missing/unknown.
pub fn parse(bytes: &[u8]) -> Result<IncomingMessage, ProtoError> {
    let value = parse_with_repair(bytes)?;
    from_value(value)
}

/// On a decode failure, replaces every `]"` with `]` and re-parses once; if that also fails, gives
/// up.
fn parse_with_repair(bytes: &[u8]) -> Result<Value, ProtoError> {
    match serde_json::from_slice(bytes) {
        Ok(value) => Ok(value),
        Err(_first) => {
            let repaired = repair_stray_bracket_quote(bytes);
            serde_json::from_slice(&repaired).map_err(|error| ProtoError::Decode(error.to_string()))
        }
    }
}

/// Replaces every `]"` with `]`. Blind, not parse-aware; it only runs after a parse has failed.
fn repair_stray_bracket_quote(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match (bytes.get(index), bytes.get(index + 1)) {
            (Some(&b']'), Some(&b'"')) => {
                out.push(b']');
                index += 2;
            }
            (Some(&byte), _) => {
                out.push(byte);
                index += 1;
            }
            (None, _) => break,
        }
    }
    out
}

fn from_value(value: Value) -> Result<IncomingMessage, ProtoError> {
    let Value::Object(map) = value else {
        return Err(ProtoError::Malformed(
            "frame is not a JSON object".to_owned(),
        ));
    };
    let action = map
        .get("action")
        .and_then(Value::as_str)
        .and_then(Action::from_wire)
        .ok_or_else(|| ProtoError::Malformed("missing or unknown action".to_owned()))?;
    Ok(IncomingMessage {
        s_id: map.get("sID").and_then(coerce_u64),
        msg_id: map.get("msgID").and_then(coerce_u64),
        resource: map
            .get("resource")
            .and_then(Value::as_str)
            .map(str::to_owned),
        version: map.get("version").and_then(Version::from_value),
        action,
        data: normalise_data(map.get("data")),
        code: map.get("code").and_then(coerce_code),
    })
}

/// A list stays a list, a single object is wrapped, `null`/absent → `None`.
fn normalise_data(value: Option<&Value>) -> Option<Vec<Value>> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::Array(items)) => Some(items.clone()),
        Some(other) => Some(vec![other.clone()]),
    }
}

/// A `code` is an error when it is non-null (`null`/absent → `None`), so `0` is kept as `Some(0)`.
fn coerce_code(value: &Value) -> Option<i64> {
    if value.is_null() {
        None
    } else {
        parse_int_from_value(value)
    }
}

fn coerce_u64(value: &Value) -> Option<u64> {
    if let Value::Number(number) = value
        && let Some(unsigned) = number.as_u64()
    {
        return Some(unsigned);
    }
    parse_int_from_value(value).and_then(|signed| u64::try_from(signed).ok())
}

/// A number as itself, or a quoted string coerced float-tolerantly.
fn parse_int_from_value(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().and_then(f64_to_i64)),
        Value::String(text) => parse_int_lenient(text),
        _ => None,
    }
}

fn parse_int_lenient(text: &str) -> Option<i64> {
    let trimmed = text.trim().trim_matches('"');
    trimmed
        .parse::<i64>()
        .ok()
        .or_else(|| trimmed.parse::<f64>().ok().and_then(f64_to_i64))
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "truncation is the intended coercion (\"4.2\" -> 4); non-finite input is filtered"
)]
fn f64_to_i64(value: f64) -> Option<i64> {
    value.is_finite().then(|| value.trunc() as i64)
}
