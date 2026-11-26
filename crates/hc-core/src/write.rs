//! The write path and its choke point.
//!
//! Kessel applies no per-appliance write policy. Whether a particular write is possible is decided
//! by the appliance's own `access` declaration ([`check_access`]) and, for HTTP, by the API token.
//!
//! [`start_command`] constructs the command that starts a programme on any appliance, including
//! the cooktop. [`GuardedWriter`] owns the byte transport in a private field with no accessor, so
//! [`GuardedWriter::write`] is the only path to [`WriteTransport::transmit`]. A second write path
//! is the way bytes would reach an appliance unintentionally; tests sweep for one.

use async_trait::async_trait;
use core::fmt;

use hc_profile::{Access, EntityKind, EntityModel};
use serde_json::Value;
use thiserror::Error;

pub use crate::device::DeviceRole;

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
    /// An optional human label for the write, for logs and the inspector.
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
    /// Whether the appliance validates this write: `false` on the direct `POST /ro/selectedProgram`
    /// start of the hob and the oven. Never a key in any frame.
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

/// The command that starts `program` on this appliance, with `options` as `(uid, value)` pairs in
/// wire order.
///
/// The path is read from the appliance's own description, as the references do it
/// (`docs/protocol/message-model.md` §6):
///
/// - a writable `<activeProgram>` (the dishwasher): `POST /ro/activeProgram`, since a selection
///   there only selects;
/// - otherwise (the hob and the oven declare it `read`): a direct `POST /ro/selectedProgram`, which
///   is their start.
///
/// "Writable" includes `access="none"`, as everywhere else on the write path. The appliance answers
/// for whether the programme starts.
#[must_use]
pub fn start_command(
    model: &EntityModel,
    program: u16,
    options: Vec<(u16, Value)>,
) -> WriteCommand {
    let via_active = model
        .entities()
        .values()
        .find(|entity| matches!(entity.kind, EntityKind::ActiveProgram))
        .and_then(|entity| entity.access.as_ref())
        .is_some_and(|access| access.is_writable() || matches!(access, Access::None));
    let resource = if via_active {
        crate::bridge::ACTIVE_PROGRAM_RESOURCE
    } else {
        crate::bridge::SELECTED_PROGRAM_RESOURCE
    };
    WriteCommand {
        resource: resource.to_owned(),
        // The programme's uid, emitted as the frame's `program` field.
        target_uid: Some(program),
        field: None,
        // For the inspector only; not a wire field on this path.
        value: Value::from(program),
        validate: via_active,
        options,
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
