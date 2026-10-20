//! The write path and its choke point.
//!
//! Kessel applies no per-appliance write policy. Whether a particular write is possible is decided
//! by the appliance's own `access` declaration ([`check_access`]) and, for HTTP, by the API token.
//!
//! [`start_command`] can construct a command that starts a programme on any appliance, including
//! the cooktop. [`GuardedWriter`] owns the byte transport in a private field with no accessor, so
//! [`GuardedWriter::write`] is the only path to [`WriteTransport::transmit`]. A second write path
//! is the way bytes would reach an appliance unintentionally; tests sweep for one.

use async_trait::async_trait;
use core::fmt;

use hc_profile::Access;
use serde_json::Value;
use thiserror::Error;

pub use crate::device::DeviceRole;

/// The washer/dryer `FinishInRelative` option uid.
///
/// It is written as a `{"uid":…,"value":…}` item, uid first, in a `POST /ro/values`.
pub const FINISH_IN_RELATIVE_UID: u16 = 551;

/// A fully-formed feature write, ready to hand to the transport.
///
/// `Debug` renders the routing metadata but never the raw value, which is appliance body content;
/// read [`WriteCommand::value`] explicitly when it is needed.
#[derive(Clone, PartialEq, Eq)]
pub struct WriteCommand {
    /// The resource the write targets, e.g. `/ro/selectedProgram` or `/ro/values`.
    pub resource: String,
    /// The target uid, when the write addresses one entity.
    pub target_uid: Option<u16>,
    /// A human label for the chosen start path (`StartInRelative` / `FinishInRelative`). `None`
    /// for a direct `selectedProgram` write.
    ///
    /// It has no wire representation: the wire addresses the option by `uid` only, and
    /// [`crate::bridge::to_outgoing`] does not consult it.
    pub field: Option<String>,
    /// The value to write. For a program start this is the program uid, and [`Self::options`] carries
    /// the options.
    pub value: Value,
    /// A program start's options, as `(uid, value)` pairs in wire order. Empty for every
    /// `/ro/values` feature write.
    ///
    /// Each pair becomes one `{"uid":…,"value":…}` item, uid first; the firmware parses key order.
    pub options: Vec<(u16, Value)>,
    /// The appliance's `validate` device-description attribute. It selects the start path (the
    /// cooktop's `selectedProgram.validate=false` means a direct `POST /ro/selectedProgram`) and is
    /// never a key in any frame.
    pub validate: bool,
}

/// Renders [`WriteCommand::value`] as a redaction marker, never its contents.
struct ValueSummary<'a>(&'a Value);

impl fmt::Debug for ValueSummary<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The value can carry appliance body content; render its JSON *type*, never its contents.
        let kind = match self.0 {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        write!(f, "<{kind} value>")
    }
}

impl fmt::Debug for WriteCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriteCommand")
            .field("resource", &self.resource)
            .field("target_uid", &self.target_uid)
            .field("field", &self.field)
            .field("value", &ValueSummary(&self.value))
            .field("validate", &self.validate)
            // Option uids are routing metadata; their values are body content and are summarised.
            .field(
                "options",
                &self
                    .options
                    .iter()
                    .map(|(uid, value)| (*uid, ValueSummary(value)))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// A request to start a program, before the device-specific start path is chosen.
#[derive(Debug, Clone)]
pub struct StartRequest {
    /// The program to start.
    pub program_uid: u16,
    /// The option uid the relative-start value is written to, from the parsed model (dishwasher
    /// `StartInRelative`). Ignored for the hob (direct `selectedProgram`) and washer/dryer (which
    /// pin [`FINISH_IN_RELATIVE_UID`]).
    pub start_option_uid: u16,
    /// The relative time in seconds (`0` = now) for `Start`/`FinishInRelative`.
    pub relative_time: i64,
}

/// Why a program cannot be started for a role.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum StartError {
    /// The role has no program-start path (the fridge has no programs).
    #[error("role {0} has no program-start path")]
    UnsupportedRole(String),
}

/// Everything the write path can refuse.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum WriteError {
    /// The feature's `access` does not permit writing.
    #[error("feature access '{0}' is not writable")]
    AccessNotWritable(String),
    /// The write did not go through: the connection failed, or the appliance refused it.
    #[error("{0}")]
    Transport(String),
}

/// The access gate: a write is permitted only if the feature's `access` is writable, or is
/// `none`. It is the only check Kessel applies; [`GuardedWriter::write`] refuses nothing.
///
/// # Errors
///
/// [`WriteError::AccessNotWritable`] when `access` is absent, read-only, or an unknown word
/// (default-deny).
pub fn check_access(access: Option<&Access>) -> Result<(), WriteError> {
    match access {
        Some(access) if access.is_writable() => Ok(()),
        // `access="none"` means the appliance declined to say, which differs from `read` (it said
        // no) and from an absent attribute. Attempt the write and let the appliance's own 400/403
        // decide: the fridge, for example, declares `none` on every setting it reports.
        Some(Access::None) => Ok(()),
        // `is_writable` is false for `Read`/`ReadStatic`/`Other(_)`, and `Access` is
        // `#[non_exhaustive]`: default-deny.
        Some(access) => Err(WriteError::AccessNotWritable(access_label(access))),
        None => Err(WriteError::AccessNotWritable("<absent>".to_owned())),
    }
}

/// A short label for an [`Access`] value, for the [`WriteError::AccessNotWritable`] message.
fn access_label(access: &Access) -> String {
    match access {
        Access::None => "none".to_owned(),
        Access::Read => "read".to_owned(),
        Access::ReadStatic => "readStatic".to_owned(),
        Access::WriteOnly => "writeOnly".to_owned(),
        Access::ReadWrite => "readWrite".to_owned(),
        Access::Other(raw) => raw.clone(),
        // `Access` is `#[non_exhaustive]`; a future variant is still refused.
        _ => "unknown".to_owned(),
    }
}

/// Chooses the device-specific program-start command. The result starts a programme on the
/// appliance once written.
///
/// # Errors
///
/// [`StartError::UnsupportedRole`] for a role with no start path.
pub fn start_command(
    role: &DeviceRole,
    request: &StartRequest,
) -> Result<WriteCommand, StartError> {
    match role {
        // Cooktop and oven: a direct `POST /ro/selectedProgram` with `validate=false`.
        // `target_uid` is the program uid, emitted as `program`; `value` repeats it for the
        // inspector and is not a wire field on this path.
        DeviceRole::Cooktop | DeviceRole::Oven => Ok(WriteCommand {
            resource: "/ro/selectedProgram".to_owned(),
            target_uid: Some(request.program_uid),
            field: None,
            value: Value::from(request.program_uid),
            validate: false,
            // `StartRequest` does not say which options a cooktop or oven program expects; a caller
            // that knows builds the `WriteCommand` directly.
            options: Vec::new(),
        }),
        // Dishwasher: `StartInRelative` on the selected program's option uid.
        DeviceRole::Dishwasher => Ok(WriteCommand {
            resource: "/ro/values".to_owned(),
            target_uid: Some(request.start_option_uid),
            field: Some("StartInRelative".to_owned()),
            value: Value::from(request.relative_time),
            validate: true,
            options: Vec::new(),
        }),
        // Washer/dryer: `FinishInRelative`, pinned to uid 551, on a write window (often
        // `access=none` until armed; the access gate handles that separately).
        DeviceRole::Washer | DeviceRole::Dryer => Ok(WriteCommand {
            resource: "/ro/values".to_owned(),
            target_uid: Some(FINISH_IN_RELATIVE_UID),
            field: Some("FinishInRelative".to_owned()),
            value: Value::from(request.relative_time),
            validate: true,
            options: Vec::new(),
        }),
        // The fridge declares no programs, and an unrecognised class has no known start path.
        DeviceRole::Fridge | DeviceRole::Other(_) => {
            Err(StartError::UnsupportedRole(role.label().to_owned()))
        }
    }
}

/// The byte transport a write is handed to: a real device connection or a test fake.
#[async_trait]
pub trait WriteTransport: Send {
    /// Transmits one write command. Called only by [`GuardedWriter::write`].
    ///
    /// # Errors
    ///
    /// A transport-specific message on failure.
    async fn transmit(&mut self, command: &WriteCommand) -> Result<(), String>;
}

/// The write choke point: one privately-owned transport, reachable one way.
///
/// The `transport` field is private and has no accessor, so [`GuardedWriter::write`] is the only path
/// to [`WriteTransport::transmit`]. Tests grep for this type name, so keep it.
pub struct GuardedWriter<T> {
    transport: T,
}

impl<T> fmt::Debug for GuardedWriter<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never render the transport: it may hold a live session and appliance body data.
        f.debug_struct("GuardedWriter").finish_non_exhaustive()
    }
}

impl<T: WriteTransport> GuardedWriter<T> {
    /// A writer over `transport`.
    pub const fn new(transport: T) -> Self {
        Self { transport }
    }

    /// Writes one command to the transport.
    ///
    /// # Errors
    ///
    /// [`WriteError::Transport`] if the transport fails.
    pub async fn write(&mut self, command: &WriteCommand) -> Result<(), WriteError> {
        // The only path to the transport.
        self.transport
            .transmit(command)
            .await
            .map_err(WriteError::Transport)
    }
}
