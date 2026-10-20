//! Device identity as the supervisor sees it: the appliance role, which selects the
//! program-start path, and the connection state published on the `connection_state` topic.

/// The appliance class.
///
/// It does not gate writes. A closed enum rather than a string because slugs, icons and the
/// program-start paths match on it exhaustively.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeviceRole {
    /// The cooktop. Programs start with a direct `POST /ro/selectedProgram`.
    Cooktop,
    /// The oven. Programs start with a direct `POST /ro/selectedProgram`.
    Oven,
    /// The dishwasher. Programs start via `StartInRelative`.
    Dishwasher,
    /// The fridge / freezer. It has no programs.
    Fridge,
    /// A washer. Programs start via `FinishInRelative` (uid 551).
    Washer,
    /// A dryer. As [`DeviceRole::Washer`].
    Dryer,
    /// A role this supervisor does not recognise. It has no program-start path.
    Other(String),
}

impl DeviceRole {
    /// A short, stable label for logs and the trace bus: a class name (`"cooktop"`), never an
    /// appliance identifier.
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::Cooktop => "cooktop",
            Self::Oven => "oven",
            Self::Dishwasher => "dishwasher",
            Self::Fridge => "fridge",
            Self::Washer => "washer",
            Self::Dryer => "dryer",
            Self::Other(raw) => raw,
        }
    }
}

/// The lifecycle state a device task publishes to `<base>/<device>/connection_state`.
///
/// `Offline` is a normal state, not an error: a powered-off appliance produces one transition
/// into `Offline` and then quiet retries, never a per-retry error log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// A connection attempt is in flight.
    Connecting,
    /// Connected at the socket layer; the handshake FSM is running.
    Handshake,
    /// The handshake completed; steady state.
    Connected,
    /// The connection dropped and a reconnect is scheduled (backoff).
    Reconnecting,
    /// The device is unreachable and retries are quiet. Not an error.
    Offline,
}

impl ConnectionState {
    /// The wire token for the `connection_state` topic.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Handshake => "handshake",
            Self::Connected => "connected",
            Self::Reconnecting => "reconnecting",
            Self::Offline => "offline",
        }
    }
}
