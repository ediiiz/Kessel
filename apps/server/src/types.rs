//! The API contract: the view types the REST and SSE surface speaks, and the `ts-rs` generator that
//! emits them into `packages/api-types/src/generated/`.
//!
//! The generated tree is byte-deterministic, because `types_are_in_sync` diffs a fresh run against
//! the committed copy: every list is explicit and sorted before emission ([`export_api_types`]). The
//! barrel `index.ts` is generator output too, so a forgotten export fails that comparison.
//!
//! No type here carries a deviceID, serial, MAC, psk or iv. A device is addressed by its slug
//! (`hc_core::mapping::DeviceId`) and described by its role class (`hc_core::DeviceRole::label`).
//! Brand and `vib` (model) are not private and are carried by [`DeviceSummary`].

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use hc_core::device::ConnectionState;
use hc_core::trace::{TraceEvent, TraceLevel as CoreTraceLevel};
use serde::{Deserialize, Serialize};
use ts_rs::{Config, ExportError, TS};

use crate::labels::LabelSource;

// =================================================================================================
// The view types.
// =================================================================================================

/// A failure, rendered for the UI. The message is operator-facing prose, never an identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ApiError {
    /// What went wrong.
    pub message: String,
}

/// Liveness plus the one number the dashboard header needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Health {
    /// The daemon's name (`"kessel"`).
    pub name: String,
    /// The crate version.
    pub version: String,
    /// How many devices the supervisor knows about.
    pub device_count: u32,
}

/// What `POST /api/restart` answers.
///
/// Carries the mechanism because the two promise different things: `supervisor` means Home Assistant
/// restarts the container; `process_exit` means the process ends and something else may restart it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct RestartOutcome {
    /// `"supervisor"` or `"process_exit"`.
    pub mechanism: String,
    /// Operator-facing prose saying what is about to happen.
    pub message: String,
}

/// Whether the caller's session cookie is currently valid; what `GET /api/session` answers.
///
/// Carries no session value or expiry: the cookie is `HttpOnly` and this must not hand back a
/// substitute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SessionView {
    /// Whether the cookie on *this* request authorises a mutating call.
    ///
    /// `true` whenever [`Self::auth_required`] is `false`: the SPA's question is "may I write?".
    pub authenticated: bool,
    /// Whether this daemon authenticates mutating requests at all.
    ///
    /// `false` is the shipped default: Home Assistant's ingress has already authenticated the browser,
    /// and the SPA renders no sign-in control.
    pub auth_required: bool,
}

/// A device's lifecycle state, mirroring `hc_core::device::ConnectionState` (which is not
/// `Serialize`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum DeviceConnectionState {
    /// A connection attempt is in flight.
    Connecting,
    /// Connected at the socket layer; the handshake FSM is running.
    Handshake,
    /// Handshake complete; steady state.
    Connected,
    /// Dropped, reconnect scheduled.
    Reconnecting,
    /// Unreachable, retries quiet. Normal, not an error.
    Offline,
}

impl From<ConnectionState> for DeviceConnectionState {
    fn from(state: ConnectionState) -> Self {
        match state {
            ConnectionState::Connecting => Self::Connecting,
            ConnectionState::Handshake => Self::Handshake,
            ConnectionState::Connected => Self::Connected,
            ConnectionState::Reconnecting => Self::Reconnecting,
            ConnectionState::Offline => Self::Offline,
        }
    }
}

/// One device, as the dashboard lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DeviceSummary {
    /// The non-identifier slug (`DeviceId`): the device's address in every route.
    pub slug: String,
    /// The role **class** (`"cooktop"`, `"dishwasher"`, …), never the appliance's identity.
    pub role: String,
    /// Whether **this appliance** exposes any writable feature at all.
    ///
    /// Computed from the mapping's command topics (`crate::state::DeviceEntry::summary`), not from
    /// the role.
    pub ever_writable: bool,
    /// Current lifecycle state.
    pub connection_state: DeviceConnectionState,
    /// Why the last connection attempt failed, when the most recent one did — `null` otherwise.
    ///
    /// Tells an appliance that is switched off from one refusing the handshake. Host-redacted before
    /// it is stored (`crate::state::DeviceEntry::last_failure`).
    pub last_failure: Option<String>,
    /// How many features the profile exposes.
    pub entity_count: u32,
    /// The appliance's brand, verbatim from the profile manifest (`"BOSCH"`, `"NEFF"`, …).
    ///
    /// Contributed name tables are keyed by brand and `vib` (`crate::labels`), so a contributor needs
    /// both to know which file to write.
    pub brand: String,
    /// The appliance's model designation — the manifest's `vib` (`"SMD8TCX01E"`, …).
    ///
    /// See [`Self::brand`] for why this is on the wire at all.
    pub vib: String,
}

/// Which layer supplied a display name — the wire mirror of [`crate::labels::LabelSource`].
///
/// On the wire so the UI can tell a contributed name from one spelled out of the identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum LabelOrigin {
    /// Contributed for this exact brand and `vib`.
    Model,
    /// Contributed for every appliance sharing the key's vendor prefix.
    Prefix,
    /// Nobody named it: the identifier, re-spelled.
    Derived,
}

impl From<LabelSource> for LabelOrigin {
    fn from(source: LabelSource) -> Self {
        match source {
            LabelSource::Model => Self::Model,
            LabelSource::Prefix => Self::Prefix,
            LabelSource::Derived => Self::Derived,
        }
    }
}

/// One addressable feature, as the dashboard renders a control for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct EntityDescriptor {
    /// The wire `uid`.
    pub uid: u32,
    /// The dotted feature key, when the `FeatureMapping` names this uid.
    pub feature_key: Option<String>,
    /// The slash path the MQTT topic contract derives from the key.
    pub feature_path: Option<String>,
    /// The declared `access` word, verbatim (`"readWrite"`, `"none"`, …).
    pub access: Option<String>,
    /// Whether the feature is writable *by access* — the access gate, not the device allowlist.
    pub writable: bool,
    /// Enum member names, when the value is an enumeration.
    ///
    /// Always the appliance's own declared members, never a friendly name: this is what a write
    /// carries. [`Self::option_labels`] is the display layer over it.
    pub options: Vec<String>,
    /// What to call this feature: the contributed name, or the identifier re-spelled (`crate::labels`).
    ///
    /// Never empty. The raw [`Self::feature_key`] is meant to be rendered beside it.
    pub label: String,
    /// Which layer produced [`Self::label`].
    pub label_source: LabelOrigin,
    /// Contributed help text for the feature, when someone wrote one.
    ///
    /// Never derived; `None` means nobody has described this feature yet.
    pub description: Option<String>,
    /// Display names for the enum members, keyed by the **raw** member name.
    ///
    /// Sparse: a member nobody named has no entry, and the UI shows the raw member.
    pub option_labels: BTreeMap<String, String>,
    /// The feature's current value as the appliance last reported it, rendered for display.
    ///
    /// An enumeration is rendered as its declared member name, so it matches one of
    /// [`Self::options`]. Everything else is its JSON value; a string without its quotes.
    ///
    /// `None` means no value: either the device has no running task, or the appliance has not
    /// reported this uid.
    pub value: Option<String>,
}

/// One of the appliance's programmes, as the dashboard lists it.
///
/// Read-only: it lets an operator see what the appliance can run and lets
/// [`EntityDescriptor::value`] on `Root.SelectedProgram` be shown as a name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ProgramDescriptor {
    /// The wire `uid`, which a `/ro/selectedProgram` write addresses.
    pub uid: u32,
    /// The dotted programme key, when the `FeatureMapping` names this uid
    /// (`Dishcare.Dishwasher.Program.Eco50`).
    pub feature_key: Option<String>,
    /// What to call it: the contributed name, or the key's last segment re-spelled (`crate::labels`).
    pub label: String,
    /// Which layer produced [`Self::label`].
    pub label_source: LabelOrigin,
    /// Contributed help text, when someone wrote one.
    pub description: Option<String>,
    /// The appliance's declared `execution` word, verbatim (`"none"`, `"selectOnly"`,
    /// `"selectAndStart"`).
    ///
    /// Sent unchanged: a start control must gate on what the appliance says it accepts.
    pub execution: Option<String>,
}

/// Where the host a connection test dialled came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum HostSource {
    /// Derived from the profile manifest.
    Derived,
    /// Supplied by the operator through the manual-IP escalation.
    Manual,
}

/// The outcome of the onboarding connection test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ConnectionTest {
    /// Connect + handshake succeeded.
    Connected,
    /// It failed. `needs_manual_ip` means the wizard must prompt for an IP.
    Failed {
        /// Operator-facing reason. Never an identifier.
        reason: String,
        /// Whether the UI must now show the manual-IP prompt.
        needs_manual_ip: bool,
    },
}

/// What a posted profile ZIP produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct OnboardingResult {
    /// The parsed device.
    pub device: DeviceSummary,
    /// How many entities the description declared but the parser could not address.
    pub skipped_entity_count: u32,
    /// Where the dialled host came from.
    pub host_source: HostSource,
    /// The connection test's verdict.
    pub connection: ConnectionTest,
}

/// What a parsed profile ZIP reports before anything is dialled (the first onboarding request).
///
/// It registers nothing, persists nothing and opens no socket, so the operator can supply the host and
/// name before a connection attempt is spent.
///
/// There is no host field: the derived host contains the `haId` on both transports, and it rarely
/// resolves without mDNS anyway. The host field starts empty and the operator fills it in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ProfilePreview {
    /// The role **class** (`"cooktop"`, `"oven"`, …), never the appliance's identity.
    pub role: String,
    /// The slug to default the dialog's name field to.
    ///
    /// For a new appliance, the first free role-derived slug; for one already onboarded, its current
    /// slug, so confirming updates it in place.
    pub suggested_slug: String,
    /// The slug this profile would **update**, if this appliance is already onboarded.
    ///
    /// Matched on `hc_profile::Profile::fingerprint`, so it holds for a re-exported archive. `None`
    /// means this is a new device.
    pub existing_slug: Option<String>,
    /// How many features the profile exposes.
    pub entity_count: u32,
    /// How many entities the description declared but the parser could not address.
    pub skipped_entity_count: u32,
    /// Which wire transport this appliance's key material selects — `"aes"` or `"tls_psk"`.
    ///
    /// Not an operator choice: `crate::live_transport::select_transport` decides it from the presence
    /// of an `iv`.
    pub transport: String,
    /// The port this transport uses absent an override — 80 on AES, 443 on TLS-PSK.
    ///
    /// Prefills the dialog's port field; defined once by `hc_proto::Transport::default_port`.
    pub default_port: u16,
}

/// The operator's confirmed choices, sent as the query string of the commit request.
///
/// Query parameters because the body is the profile ZIP, re-posted rather than held server-side
/// between the two requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CommitRequest {
    /// What to call this device: its MQTT topic root, API address and profile-store filename, so
    /// `crate::store::valid_slug` gates it.
    pub slug: String,
    /// The appliance's address on the operator's network.
    ///
    /// `None` falls back to the profile-derived host. The UI always sends one.
    pub host: Option<String>,
    /// The port override; `None` is [`ProfilePreview::default_port`].
    pub port: Option<u16>,
}

/// The manual-IP body sent after a failed connection test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ManualHostRequest {
    /// The host or IP the operator typed.
    pub host: String,
}

/// Which multiplexed SSE `event:` a frame is delivered on.
///
/// One stream, several `event:` types, so the browser stays under the HTTP/1.1 per-origin
/// connection cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SseEventKind {
    /// A `ConnectionState` transition.
    Connection,
    /// A frame the inspector renders (direction, hex, decoded JSON, handshake step, chain state).
    Frame,
    /// The appliance reported a new value for one uid.
    Value,
    /// Anything else on the trace bus.
    Log,
}

impl SseEventKind {
    /// The `event:` field name on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Connection => "connection",
            Self::Frame => "frame",
            Self::Value => "value",
            Self::Log => "log",
        }
    }
}

/// Severity, mirroring `hc_core::trace::TraceLevel` (which does not derive `TS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TraceLevel {
    /// Something failed.
    Error,
    /// Recovered from something unexpected.
    Warn,
    /// A lifecycle milestone.
    Info,
    /// Diagnostic detail.
    Debug,
    /// The per-frame firehose.
    Trace,
}

impl From<CoreTraceLevel> for TraceLevel {
    fn from(level: CoreTraceLevel) -> Self {
        match level {
            CoreTraceLevel::Error => Self::Error,
            CoreTraceLevel::Warn => Self::Warn,
            CoreTraceLevel::Info => Self::Info,
            CoreTraceLevel::Debug => Self::Debug,
            CoreTraceLevel::Trace => Self::Trace,
        }
    }
}

impl TraceLevel {
    /// Every level, most severe first — the order a threshold control offers them in.
    pub const ALL: [Self; 5] = [
        Self::Error,
        Self::Warn,
        Self::Info,
        Self::Debug,
        Self::Trace,
    ];

    /// The name on the wire, matching the `snake_case` serde rename above.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }

    /// Parses the wire name; `None` for anything else.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|level| level.as_str() == text)
    }

    /// How verbose this level is: `Error` is 0 and `Trace` is 4.
    ///
    /// Named rather than derived from `Ord`, because a bare `<=` on severity is easy to get backwards.
    #[must_use]
    pub const fn verbosity(self) -> u8 {
        match self {
            Self::Error => 0,
            Self::Warn => 1,
            Self::Info => 2,
            Self::Debug => 3,
            Self::Trace => 4,
        }
    }

    /// Whether an event at this level survives a `level=<threshold>` filter — "threshold and
    /// worse", the way every log level control reads.
    #[must_use]
    pub const fn admits(self, threshold: Self) -> bool {
        self.verbosity() <= threshold.verbosity()
    }
}

/// Which subsystem an event came from, derived from its `tracing` target.
///
/// The inspector's second filter, as a small fixed set of groups. [`SOURCE_PREFIXES`] is the whole
/// mapping; a target matching no prefix is [`TraceSource::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TraceSource {
    /// The appliance protocol: the `hc-*` crates (transport, framing, crypto, profile).
    ///
    /// Renamed explicitly to match [`TraceSource::as_str`]; `snake_case` would give `home_connect`.
    #[serde(rename = "homeconnect")]
    HomeConnect,
    /// The MQTT bridge to Home Assistant, and the client crate under it.
    Mqtt,
    /// The HTTP surface the SPA talks to: `axum`, `tower`, `hyper`.
    Http,
    /// The daemon's own lifecycle — everything else this workspace emits.
    Daemon,
    /// A target belonging to no group above.
    Other,
}

/// Target prefixes, longest first, and the group each one maps to.
///
/// Longest first, so `server::mqtt` wins over `server`.
pub const SOURCE_PREFIXES: &[(&str, TraceSource)] = &[
    ("server::mqtt", TraceSource::Mqtt),
    ("hc_transport", TraceSource::HomeConnect),
    ("hc_downloader", TraceSource::HomeConnect),
    ("hc_profile", TraceSource::HomeConnect),
    ("hc_crypto", TraceSource::HomeConnect),
    ("tower_http", TraceSource::Http),
    ("hc_proto", TraceSource::HomeConnect),
    ("hc_core", TraceSource::HomeConnect),
    ("rumqttc", TraceSource::Mqtt),
    ("kessel", TraceSource::Daemon),
    ("server", TraceSource::Daemon),
    ("hyper", TraceSource::Http),
    ("tower", TraceSource::Http),
    ("axum", TraceSource::Http),
];

impl TraceSource {
    /// Every group, in the order the UI offers them.
    pub const ALL: [Self; 5] = [
        Self::HomeConnect,
        Self::Mqtt,
        Self::Http,
        Self::Daemon,
        Self::Other,
    ];

    /// The name on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HomeConnect => "homeconnect",
            Self::Mqtt => "mqtt",
            Self::Http => "http",
            Self::Daemon => "daemon",
            Self::Other => "other",
        }
    }

    /// Parses the wire name; `None` for anything else.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|source| source.as_str() == text)
    }

    /// The group a `tracing` target belongs to.
    ///
    /// Matched on a path-segment boundary, so a hypothetical `serverside::x` is not swallowed by
    /// the `server` prefix.
    #[must_use]
    pub fn of_target(target: &str) -> Self {
        for &(prefix, source) in SOURCE_PREFIXES {
            let boundary = target
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"));
            if boundary {
                return source;
            }
        }
        Self::Other
    }
}

/// One event as the frame inspector receives it.
///
/// The fields are the trace bus's already-rendered strings; every value passed through `Secret<T>`'s
/// redacting `Debug` on its way in, so secrets cannot reach this surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TraceFrame {
    /// The bus-assigned sequence number: monotonic and gap-free, so a gap means "dropped".
    pub seq: u64,
    /// When the daemon recorded the event, in milliseconds since the Unix epoch.
    pub timestamp_ms: u64,
    /// Which `event:` this frame was delivered on.
    pub kind: SseEventKind,
    /// Severity.
    pub level: TraceLevel,
    /// The `tracing` target.
    pub target: String,
    /// Which subsystem [`target`](Self::target) belongs to, so the SPA can group and filter without
    /// re-deriving the mapping in TypeScript.
    pub source: TraceSource,
    /// The event message.
    pub message: String,
    /// The remaining structured fields, rendered.
    pub fields: BTreeMap<String, String>,
    /// Whether this frame came out of the retained ring buffer rather than off the live feed.
    ///
    /// Every new subscriber receives the whole retained buffer, so a client that reacts to events
    /// (`$lib/live`) uses this to ignore old connection transitions. It describes this delivery, not
    /// the event.
    pub replayed: bool,
}

impl TraceFrame {
    /// Classifies and converts a bus event into the frame the SSE stream delivers **live**.
    ///
    /// [`Self::from_retained`] is the same conversion for a frame coming out of the ring buffer.
    #[must_use]
    pub fn from_event(event: TraceEvent) -> Self {
        let kind = classify(&event);
        Self {
            seq: event.seq,
            timestamp_ms: event.timestamp_ms,
            kind,
            level: event.level.into(),
            source: TraceSource::of_target(&event.target),
            target: event.target,
            message: event.message,
            fields: event.fields,
            replayed: false,
        }
    }

    /// The same conversion for an event replayed out of the retained ring buffer.
    #[must_use]
    pub fn from_retained(event: TraceEvent) -> Self {
        Self {
            replayed: true,
            ..Self::from_event(event)
        }
    }
}

/// The `event:` type a bus event is multiplexed onto.
///
/// Keyed off marker fields rather than message text: a frame carries `direction`, a supervisor
/// transition carries `state` (`hc_core::supervisor::emit_state`), a value update carries
/// [`VALUE_FIELD`]. Anything else is a log line.
#[must_use]
pub fn classify(event: &TraceEvent) -> SseEventKind {
    if event.fields.contains_key(FRAME_FIELD) {
        SseEventKind::Frame
    } else if event.fields.contains_key(STATE_FIELD) {
        SseEventKind::Connection
    } else if event.fields.contains_key(VALUE_FIELD) {
        SseEventKind::Value
    } else {
        SseEventKind::Log
    }
}

/// The field whose presence marks an event as a *frame* for the inspector.
pub const FRAME_FIELD: &str = "direction";

/// The field whose presence marks an event as a *connection-state transition*.
pub const STATE_FIELD: &str = "state";

/// The field whose presence marks an event as a feature-value update.
///
/// A dedicated marker rather than `uid`, which could plausibly appear on an ordinary log line.
pub const VALUE_FIELD: &str = "feature_value";

/// The field carrying the device slug on a value event (a slug, never an identifier).
pub const VALUE_DEVICE_FIELD: &str = "device";

/// The field carrying the feature uid on a value event, rendered as a decimal string.
pub const VALUE_UID_FIELD: &str = "uid";

/// What a write request asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct WriteRequest {
    /// The dotted feature key to write.
    pub feature_key: String,
    /// The value, as the UI collected it: a number, a boolean, or — for an enumeration — either a
    /// declared member name or the declared wire ordinal the appliance itself reports, so a value
    /// read from a device can be written straight back.
    pub value: String,
}

/// What the write choke point decided (`hc_core::write::GuardedWriter`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum WriteOutcome {
    /// The guard allowed it and the transport accepted it.
    Accepted,
    /// The guard, the access gate, or the transport refused.
    Refused {
        /// Why. Operator-facing prose.
        reason: String,
    },
}

// =================================================================================================
// The generator.
// =================================================================================================

/// The barrel, which `ts-rs` does not emit.
const BARREL_FILE: &str = "index.ts";

/// The barrel's header. Part of generator output, so it is inside `types_are_in_sync`'s comparison.
const BARREL_HEADER: &str = "\
// This file was generated by `server::types::export_api_types`. Do not edit it\n\
// manually: `types_are_in_sync` diffs it against a fresh generator run, so a hand edit is a red\n\
// gate. Export order is lexicographic by file name — see `packages/api-types/README.md`.\n\
\n";

/// Calls [`emit_one`] for each named type. A macro because the types are heterogeneous: there is no
/// `dyn TS`, and a `Vec` of them is not expressible.
macro_rules! emit_all {
    ($cfg:expr, $dir:expr, $files:expr, $($ty:ty),+ $(,)?) => {
        $( emit_one::<$ty>($cfg, $dir, $files)?; )+
    };
}

/// Emits every API type plus the barrel into `dir`, and returns the file names written, sorted.
///
/// Deterministic by construction: the type list below is explicit (not a `HashMap` walk), the file
/// names are sorted before the barrel is written, and every byte comes from `ts-rs`'s
/// `export_to_string` under a fixed [`Config`].
///
/// # Errors
///
/// [`ExportError`] if a type cannot be rendered or a file cannot be written.
pub fn export_api_types(dir: &Path) -> Result<Vec<String>, ExportError> {
    std::fs::create_dir_all(dir)?;
    let cfg = config(dir);
    let mut files = Vec::new();

    emit_all!(
        &cfg,
        dir,
        &mut files,
        ApiError,
        // The configuration types live in `crate::config`, beside their storage logic.
        crate::config::ConfigUpdate,
        crate::config::ConfigView,
        CommitRequest,
        ConnectionTest,
        DeviceConnectionState,
        DeviceSummary,
        EntityDescriptor,
        Health,
        HostSource,
        LabelOrigin,
        ManualHostRequest,
        crate::config::MqttBrokerView,
        crate::config::MqttSource,
        OnboardingResult,
        ProfilePreview,
        ProgramDescriptor,
        RestartOutcome,
        SessionView,
        SseEventKind,
        TraceFrame,
        TraceLevel,
        TraceSource,
        WriteOutcome,
        WriteRequest,
    );

    files.sort();
    std::fs::write(dir.join(BARREL_FILE), barrel(&files))?;
    files.push(BARREL_FILE.to_owned());
    Ok(files)
}

/// The one `ts-rs` configuration both the generator and the committed tree are produced under.
///
/// * `import_extension = "js"`: the SPA's `verbatimModuleSyntax` + `moduleResolution: bundler`
///   setup wants `./Foo.js` specifiers.
/// * `large_int = "number"` — `u64` would otherwise be typed `bigint`, which `JSON.parse` never
///   produces. The only `u64` on this surface is [`TraceFrame::seq`], a counter far below 2^53.
fn config(dir: &Path) -> Config {
    Config::new()
        .with_out_dir(dir)
        .with_import_extension(Some("js"))
        .with_large_int("number")
}

/// Renders the barrel from the sorted file list.
fn barrel(files: &[String]) -> String {
    let mut out = String::from(BARREL_HEADER);
    for file in files {
        let stem = file.strip_suffix(".ts").unwrap_or(file);
        // `writeln!` into a `String` is infallible; the `Result` is discarded deliberately.
        let _ = writeln!(out, "export type * from './{stem}.js';");
    }
    out
}

/// Writes one type's file, recording its name. The file name is `ts-rs`'s own
/// (`<TypeName>.ts`), so the layout stays the crate's default shape.
fn emit_one<T: TS + 'static + ?Sized>(
    cfg: &Config,
    dir: &Path,
    files: &mut Vec<String>,
) -> Result<(), ExportError> {
    let relative: PathBuf = <T as TS>::output_path()
        .ok_or_else(|| ExportError::CannotBeExported(std::any::type_name::<T>()))?;
    let name = relative
        .to_str()
        .ok_or_else(|| ExportError::CannotBeExported(std::any::type_name::<T>()))?
        .to_owned();
    std::fs::write(dir.join(&relative), <T as TS>::export_to_string(cfg)?)?;
    files.push(name);
    Ok(())
}
