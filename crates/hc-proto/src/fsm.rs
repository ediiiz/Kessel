//! The handshake state machine.
//!
//! Pure and event-driven: [`Handshake::on_message`] takes one inbound [`IncomingMessage`] and
//! returns the [`OutgoingMessage`]s to send, advancing state. It performs no I/O and holds no
//! crypto; the [`crate::Driver`] wraps it with a [`crate::Socket`] and the `hc-crypto` chain.
//!
//! - `/ci/authentication` and `/ci/info` are sent when the `ci` service version is `< 3` and
//!   skipped otherwise.
//! - `/ei/deviceReady` is sent unconditionally.
//!
//! The RESPONSE to `/ei/initialValues` reuses the server's `msgID` and consumes none; the client
//! counter starts at the `edMsgID` from that POST and increments once per subsequent send.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::error::ProtoError;
use crate::message::{
    Action, DeviceIdentity, DeviceType, IncomingMessage, OutgoingMessage, Version, service_key,
};

/// The `ci` service version at or above which `/ci/authentication` + `/ci/info` are skipped.
const CI_AUTH_SKIP_VERSION: i64 = 3;

/// The source of the `/ci/authentication` nonce token: `base64url(32 random bytes)`, unpadded.
///
/// It yields the finished string rather than raw bytes so a replay test can supply a recorded
/// token verbatim, even one that is not a canonical encoding of 32 bytes.
pub trait NonceSource {
    /// The base64url nonce token for one `/ci/authentication`.
    fn next_nonce(&mut self) -> String;
}

/// Production nonce source: `base64url(32 bytes)` from the thread RNG, `=` stripped.
#[derive(Debug, Default)]
pub struct RandomNonce;

impl NonceSource for RandomNonce {
    fn next_nonce(&mut self) -> String {
        use rand::RngCore as _;
        let mut raw = [0_u8; 32];
        rand::rng().fill_bytes(&mut raw);
        URL_SAFE_NO_PAD.encode(raw)
    }
}

/// Static configuration for a handshake: the client identity, a fixed constant rather than
/// per-connection random.
#[derive(Debug, Clone)]
pub struct HandshakeConfig {
    /// The client's `deviceName`/`deviceID`.
    pub identity: DeviceIdentity,
}

/// The handshake's progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Waiting for the device's opening `POST /ei/initialValues`.
    AwaitingInitialValues,
    /// The `GET /ci/services` is outstanding; its response populates the service table.
    AwaitingServices,
    /// The request burst is out; awaiting the core responses that gate CONNECTED.
    Handshaking,
    /// All core responses received; the session is up.
    Connected,
    /// A fatal `code` on a non-tolerant step (`/ci/services`, `/ci/authentication`).
    Failed,
}

/// The one outbound `msgID` allocator for one connection.
///
/// The counter starts at the `edMsgID` the appliance supplies in `/ei/initialValues` and
/// increments by one per outbound message that needs a fresh id. The handshake and the write path
/// must spend from the same counter: a re-used `msgID` collides with a live correlation slot.
///
/// Every clone shares one `AtomicU64`, and the restart at `edMsgID` is visible to handles taken
/// before or after it. There is no public constructor; the only source is [`Handshake::msg_ids`].
pub struct MsgIdAllocator {
    next: Arc<AtomicU64>,
}

impl Clone for MsgIdAllocator {
    /// Clones the handle, not the counter: every clone spends from one cell.
    fn clone(&self) -> Self {
        Self {
            next: Arc::clone(&self.next),
        }
    }
}

impl fmt::Debug for MsgIdAllocator {
    /// Renders the next position; a `msgID` is a correlation counter, not an identifier.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MsgIdAllocator")
            .field("next", &self.peek())
            .finish()
    }
}

impl MsgIdAllocator {
    /// A fresh allocator positioned at `start`.
    ///
    /// Private so no caller can build a second, independent counter for a connection.
    fn starting_at(start: u64) -> Self {
        Self {
            next: Arc::new(AtomicU64::new(start)),
        }
    }

    /// Repositions the shared cell at the appliance's `edMsgID`. Called once per connection, on
    /// the opening `POST /ei/initialValues`; every existing handle sees it.
    fn restart_at(&self, start: u64) {
        self.next.store(start, Ordering::SeqCst);
    }

    /// Spends the current id and advances the shared cell.
    ///
    /// Saturates rather than wraps: wrapping would restart at `0` and collide with the live
    /// sequence.
    #[must_use]
    pub fn take(&self) -> u64 {
        self.next
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                Some(current.saturating_add(1))
            })
            // The closure never returns `None`; the `Err` arm is unreachable.
            .unwrap_or_else(|current| current)
    }

    /// The id the next [`take`](Self::take) would hand out, without spending it.
    ///
    /// Do not peek, build a frame and then take: the cell is shared, so the only race-free order
    /// is take-then-build.
    #[must_use]
    pub fn peek(&self) -> u64 {
        self.next.load(Ordering::SeqCst)
    }
}

/// The handshake FSM.
///
/// `Debug` is hand-written because `notifications` can carry a decoded `/ci/info` body with
/// device identifiers.
pub struct Handshake {
    config: HandshakeConfig,
    state: State,
    s_id: u64,
    /// The connection's shared outbound `msgID` allocator; see [`MsgIdAllocator`].
    msg_ids: MsgIdAllocator,
    init_version: Version,
    services: BTreeMap<String, i64>,
    /// Resources whose response still gates CONNECTED (auth/info/iz/ni GETs, never the `/ro` bulk).
    awaited: BTreeSet<String>,
    /// The `/ro` bulk GETs whose responses have not arrived yet. They answer the handshake, not a
    /// steady-state request, even when they land after CONNECTED.
    snapshot: BTreeSet<String>,
    saw_403: bool,
    notifications: Vec<IncomingMessage>,
    /// The `/ro/*` RESPONSE messages this connection received, in arrival order: the appliance's
    /// current values.
    ///
    /// These responses carry the initial state, and an idle appliance may push no NOTIFY at all,
    /// so this snapshot can be the only source of values. Only `/ro/` resources are retained;
    /// `/ei` and `/ci` bodies (`/ci/info` carries identifiers) are dropped.
    ro_values: Vec<IncomingMessage>,
}

impl fmt::Debug for Handshake {
    /// Hand-written so a future field carrying a decoded body cannot silently reach a `{:?}`.
    ///
    /// `notifications` delegates to [`IncomingMessage`]'s redacting `Debug`. `config.identity` is
    /// the client's own identity, not the appliance's, so it is rendered.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handshake")
            .field("config", &self.config)
            .field("state", &self.state)
            .field("s_id", &self.s_id)
            .field("msg_ids", &self.msg_ids)
            .field("init_version", &self.init_version)
            .field("services", &self.services)
            .field("awaited", &self.awaited)
            .field("snapshot", &self.snapshot)
            .field("saw_403", &self.saw_403)
            .field("notifications", &self.notifications)
            // A count, never the messages: this field exists to hold appliance values.
            .field("ro_values", &self.ro_values.len())
            .finish()
    }
}

impl Handshake {
    /// A fresh FSM, waiting for the device to speak first.
    #[must_use]
    pub fn new(config: HandshakeConfig) -> Self {
        Self {
            config,
            state: State::AwaitingInitialValues,
            s_id: 0,
            // Restarted at the appliance's `edMsgID` by `/ei/initialValues`; nothing is emitted
            // before that.
            msg_ids: MsgIdAllocator::starting_at(0),
            init_version: Version::Int(2),
            services: BTreeMap::new(),
            awaited: BTreeSet::new(),
            snapshot: BTreeSet::new(),
            saw_403: false,
            notifications: Vec::new(),
            ro_values: Vec::new(),
        }
    }

    /// The current state.
    #[must_use]
    pub const fn state(&self) -> State {
        self.state
    }

    /// The connection's `sID`, assigned by the appliance in its opening `POST /ei/initialValues`.
    /// `0` before that message arrives.
    #[must_use]
    pub const fn s_id(&self) -> u64 {
        self.s_id
    }

    /// A handle on the connection's shared outbound `msgID` allocator.
    ///
    /// Hand it to the write path: a writer must spend ids from the same cell, not hold a copy of
    /// the position, or it will re-issue ids the handshake has already used.
    #[must_use]
    pub fn msg_ids(&self) -> MsgIdAllocator {
        self.msg_ids.clone()
    }

    /// The negotiated `version` for a resource: the version of its service (the two characters
    /// after the leading `/`), defaulting to `1`.
    ///
    /// Before `/ci/services` has answered, every resource resolves to `1`.
    #[must_use]
    pub fn version_for(&self, resource: &str) -> Version {
        let version = service_key(resource).map_or(1, |key| self.service_version(key));
        Version::Int(version)
    }

    /// Whether any handshake response carried `code == 403`.
    #[must_use]
    pub const fn saw_403(&self) -> bool {
        self.saw_403
    }

    /// The NOTIFYs and uncorrelated messages received so far and not yet drained.
    #[must_use]
    pub fn notifications(&self) -> &[IncomingMessage] {
        &self.notifications
    }

    /// Drains the NOTIFY buffer: returns everything accumulated since the last drain and clears it.
    ///
    /// The connection's steady-state loop should be the only drainer; other readers use
    /// [`Self::notifications`]. Without a drain the buffer grows for the life of the connection.
    ///
    /// The returned messages carry appliance body data: decode and publish the values, never log
    /// the messages.
    #[must_use]
    pub fn take_notifications(&mut self) -> Vec<IncomingMessage> {
        core::mem::take(&mut self.notifications)
    }

    /// The `/ro/*` RESPONSE messages this connection received: the appliance's current values.
    ///
    /// These carry appliance body data: decode and publish the values, never log the messages.
    #[must_use]
    pub fn ro_values(&self) -> &[IncomingMessage] {
        &self.ro_values
    }

    /// Advances the handshake with one inbound message.
    ///
    /// # Errors
    ///
    /// [`ProtoError::Malformed`] on a broken opening frame; [`ProtoError::Response`] on a fatal
    /// `code` (`/ci/services`, `/ci/authentication`); [`ProtoError::Unexpected`] if the first frame
    /// is not `POST /ei/initialValues`.
    pub fn on_message(
        &mut self,
        message: IncomingMessage,
        nonce: &mut dyn NonceSource,
    ) -> Result<Vec<OutgoingMessage>, ProtoError> {
        match self.state {
            State::AwaitingInitialValues => self.on_initial_values(&message),
            State::AwaitingServices => self.on_awaiting_services(message, nonce),
            State::Handshaking => {
                if message.action == Action::Response {
                    self.on_response(&message)?;
                } else {
                    self.notifications.push(message);
                }
                Ok(Vec::new())
            }
            // After CONNECTED, a response that is not part of the snapshot answers one request
            // (usually a write), so an error code is that request's outcome, not the connection's.
            State::Connected => {
                let answers_snapshot = message.action == Action::Response
                    && message
                        .resource
                        .as_ref()
                        .is_some_and(|resource| self.snapshot.contains(resource));
                if answers_snapshot {
                    self.on_response(&message)?;
                } else {
                    self.notifications.push(message);
                }
                Ok(Vec::new())
            }
            State::Failed => Ok(Vec::new()),
        }
    }

    fn on_initial_values(
        &mut self,
        message: &IncomingMessage,
    ) -> Result<Vec<OutgoingMessage>, ProtoError> {
        if message.action != Action::Post
            || message.resource.as_deref() != Some("/ei/initialValues")
        {
            return Err(ProtoError::Unexpected {
                action: message.action.to_string(),
                resource: message.resource.clone().unwrap_or_default(),
                state: "AwaitingInitialValues".to_owned(),
            });
        }
        let s_id = message
            .s_id
            .ok_or_else(|| ProtoError::Malformed("initialValues without sID".to_owned()))?;
        let server_msg_id = message
            .msg_id
            .ok_or_else(|| ProtoError::Malformed("initialValues without msgID".to_owned()))?;
        let ed_msg_id = edge_msg_id(message).ok_or_else(|| {
            ProtoError::Malformed("initialValues without data[0].edMsgID".to_owned())
        })?;

        self.s_id = s_id;
        // The client counter starts at the appliance's `edMsgID`.
        self.msg_ids.restart_at(ed_msg_id);
        self.init_version = message.version.clone().unwrap_or(Version::Int(2));
        let ei_version = self.init_version.as_int().unwrap_or(2);

        // The RESPONSE reuses the server's msgID and consumes no client id.
        let response = OutgoingMessage::initial_values_response(
            self.s_id,
            server_msg_id,
            self.init_version.clone(),
            DeviceType::for_ei_version(ei_version),
            &self.config.identity,
        );
        // GET /ci/services is always version 1 and takes the first client msgID.
        let services = OutgoingMessage::bare(
            self.s_id,
            self.take_msg_id(),
            "/ci/services",
            Version::Int(1),
            Action::Get,
        );
        self.state = State::AwaitingServices;
        Ok(vec![response, services])
    }

    fn on_awaiting_services(
        &mut self,
        message: IncomingMessage,
        nonce: &mut dyn NonceSource,
    ) -> Result<Vec<OutgoingMessage>, ProtoError> {
        let is_services = message.action == Action::Response
            && message.resource.as_deref() == Some("/ci/services");
        if !is_services {
            // NOTIFY (e.g. the unsolicited /ci/registeredDevices) or any uncorrelated message.
            self.notifications.push(message);
            return Ok(Vec::new());
        }
        if let Some((code, name)) = message.response_error() {
            // Any code on /ci/services is fatal.
            if code == 403 {
                self.saw_403 = true;
            }
            self.state = State::Failed;
            return Err(ProtoError::Response {
                resource: "/ci/services".to_owned(),
                code,
                name,
            });
        }
        self.store_services(&message);
        Ok(self.build_burst(nonce))
    }

    fn store_services(&mut self, message: &IncomingMessage) {
        let Some(data) = message.data.as_ref() else {
            return;
        };
        for entry in data {
            if let Some(service) = entry.get("service").and_then(serde_json::Value::as_str) {
                let version = entry
                    .get("version")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(1);
                self.services.insert(service.to_owned(), version);
            }
        }
    }

    fn build_burst(&mut self, nonce: &mut dyn NonceSource) -> Vec<OutgoingMessage> {
        let mut burst = Vec::new();
        let ci = self.service_version("ci");
        if ci < CI_AUTH_SKIP_VERSION {
            burst.push(self.make_authentication(nonce));
            burst.push(self.make_get("/ci/info"));
            self.awaited.insert("/ci/authentication".to_owned());
            self.awaited.insert("/ci/info".to_owned());
        }
        if self.services.contains_key("iz") {
            burst.push(self.make_get("/iz/info"));
            self.awaited.insert("/iz/info".to_owned());
        }
        // Unconditional, fire-and-forget NOTIFY.
        burst.push(self.make_notify("/ei/deviceReady"));
        if self.services.contains_key("ni") {
            burst.push(self.make_get("/ni/info"));
            self.awaited.insert("/ni/info".to_owned());
        }
        // Mandatory values, then description changes. Post-connect bulk: these do not gate
        // CONNECTED, so they are not in `awaited`.
        for resource in ["/ro/allMandatoryValues", "/ro/allDescriptionChanges"] {
            burst.push(self.make_get(resource));
            self.snapshot.insert(resource.to_owned());
        }

        self.state = if self.awaited.is_empty() {
            State::Connected
        } else {
            State::Handshaking
        };
        burst
    }

    fn on_response(&mut self, message: &IncomingMessage) -> Result<(), ProtoError> {
        let resource = message.resource.clone().unwrap_or_default();
        // Retain the `/ro` snapshot before the fatal-code check, so a partially usable response is
        // not lost to an error return.
        if resource.starts_with("/ro/") {
            self.ro_values.push(message.clone());
        }
        if let Some((code, name)) = message.response_error() {
            if code == 403 {
                self.saw_403 = true;
            }
            if is_fatal_code(&resource, code) {
                self.state = State::Failed;
                return Err(ProtoError::Response {
                    resource,
                    code,
                    name,
                });
            }
            tracing::debug!(%resource, code, "tolerated code on a non-fatal handshake step");
        }
        self.awaited.remove(&resource);
        self.snapshot.remove(&resource);
        if self.awaited.is_empty() && self.state == State::Handshaking {
            self.state = State::Connected;
        }
        Ok(())
    }

    fn make_authentication(&self, nonce: &mut dyn NonceSource) -> OutgoingMessage {
        let token = nonce.next_nonce();
        OutgoingMessage::authentication(
            self.s_id,
            self.take_msg_id(),
            self.version_for("/ci/authentication"),
            token,
        )
    }

    fn make_get(&self, resource: &str) -> OutgoingMessage {
        OutgoingMessage::bare(
            self.s_id,
            self.take_msg_id(),
            resource,
            self.version_for(resource),
            Action::Get,
        )
    }

    fn make_notify(&self, resource: &str) -> OutgoingMessage {
        OutgoingMessage::bare(
            self.s_id,
            self.take_msg_id(),
            resource,
            self.version_for(resource),
            Action::Notify,
        )
    }

    /// Spends one id from the connection's shared allocator.
    fn take_msg_id(&self) -> u64 {
        self.msg_ids.take()
    }

    fn service_version(&self, key: &str) -> i64 {
        self.services.get(key).copied().unwrap_or(1)
    }
}

/// `data[0].edMsgID`, int-coerced.
fn edge_msg_id(message: &IncomingMessage) -> Option<u64> {
    message
        .data
        .as_ref()
        .and_then(|data| data.first())
        .and_then(|first| first.get("edMsgID"))
        .and_then(serde_json::Value::as_u64)
}

/// `/ci/authentication` is fatal on any code; the `/ro` bulk tolerates `>= 500` and is fatal below
/// it; everything else here (`/ci/info`, `/iz/info`, `/ni/info`) is tolerated.
fn is_fatal_code(resource: &str, code: i64) -> bool {
    if resource == "/ci/authentication" {
        return true;
    }
    if resource.starts_with("/ro/") {
        return code < 500;
    }
    false
}
