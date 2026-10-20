//! The MQTT bridge.
//!
//! It ties a device's [`DeviceMapping`] to an [`MqttClient`]: discovery configs, raw state topics,
//! `…/set` subscriptions, `availability` (LWT) and `connection_state`. An incoming `…/set` is
//! routed through [`GuardedWriter`], the only path from MQTT to a device write; the real
//! `hc_transport::Sender` is moved into the guard by [`guarded_session_writer`] and no other
//! handle to it survives.
//!
//! The bridge holds no shadow: each device task owns its own. Every topic is
//! `<base>/<device>/…`, where `<device>` is the [`DeviceId`] slug, never an appliance identifier.

use async_trait::async_trait;
use thiserror::Error;

use hc_profile::{Entity, EntityModel, EnumType, ValueType};
use hc_proto::{MsgIdAllocator, OutgoingMessage, Version, WriteItem};
use hc_transport::{FrameDecoder, FrameEncoder, FrameSink, FrameStream, Sender, Session};
use serde_json::Value;

use crate::acks::WriteAcks;
use crate::device::ConnectionState;
use crate::discovery::{Announcement, device_discovery};
use crate::mapping::{DeviceId, DeviceMapping};
use crate::mqtt::{LastWill, MqttClient, MqttError};
use crate::write::{GuardedWriter, WriteCommand, WriteError, WriteTransport};

/// The `resource` a generic feature write is sent to, with verb `POST`.
const VALUES_RESOURCE: &str = "/ro/values";

/// The program-select resource. A command aimed here becomes a program-start frame
/// (`hc_proto::Body::Program`), never a `/ro/values` write.
const SELECTED_PROGRAM_RESOURCE: &str = "/ro/selectedProgram";

/// The active-program resource. A command aimed here is a `DELETE`, which aborts the running
/// programme; it is the only write that carries no body and addresses no uid.
const ACTIVE_PROGRAM_RESOURCE: &str = "/ro/activeProgram";

/// The device-relative `…/set` topic of the synthetic stop control (`crate::discovery`).
///
/// It backs no feature, so it is subscribed and routed explicitly. Feature paths never begin with
/// an underscore, so `_control/` cannot collide with one.
pub const STOP_PROGRAM_TOPIC: &str = "_control/stop_program";

/// Home Assistant's own status topic, where it publishes its birth (`online`) and will (`offline`).
///
/// HA expects integrations to re-announce discovery on its birth message. Retained configs cover
/// an ordinary restart, but not HA being installed or reconfigured after Kessel last announced.
pub const HA_STATUS_TOPIC: &str = "homeassistant/status";

/// The payload Home Assistant publishes on [`HA_STATUS_TOPIC`] when it comes up.
pub const HA_BIRTH_PAYLOAD: &[u8] = b"online";

/// Whether an inbound message is Home Assistant announcing that it has come up.
///
/// Topic and payload must both match exactly: the same topic carries HA's will and retained clears,
/// and `homeassistant/status/anything` is a different topic.
#[must_use]
pub fn is_home_assistant_birth(topic: &str, payload: &[u8]) -> bool {
    topic == HA_STATUS_TOPIC && payload == HA_BIRTH_PAYLOAD
}

/// The retained payload for an online device on its `availability` topic.
pub const AVAILABILITY_ONLINE: &[u8] = b"online";
/// The retained payload for an offline device; also the LWT payload.
pub const AVAILABILITY_OFFLINE: &[u8] = b"offline";

/// Home Assistant's "this entity has no value" sentinel on a state topic.
///
/// HA checks for it before validating against the entity's `options`; any other payload that is not
/// an option, `null` included, is discarded and the entity keeps its last value.
pub const PAYLOAD_NONE: &[u8] = b"None";

/// Why an incoming `…/set` could not be routed to a device write.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum SetError {
    /// No writable feature has this `…/set` topic.
    #[error("no writable feature for set topic '{0}'")]
    UnknownTopic(String),
    /// The topic's feature is not present in the model (mapping/model out of sync).
    #[error("no model entity for uid {0:#06X}")]
    UnknownEntity(u16),
    /// An enumeration payload that is neither a declared member label nor a declared wire ordinal
    /// of the feature's enumeration ([`enum_wire_value`]).
    #[error(
        "enum value '{0}' is neither a declared member name nor a declared value of this feature"
    )]
    EnumLabelUnknown(String),
    /// The payload was not a value this feature accepts.
    #[error("invalid set payload: {0}")]
    InvalidPayload(String),
    /// The write failed (see [`WriteError`]).
    #[error(transparent)]
    Write(#[from] WriteError),
}

/// The MQTT bridge for one device.
///
/// It owns no transport, shadow or guard; those are passed in per call, so the bridge cannot
/// become a second path to a device write.
#[derive(Debug, Clone)]
pub struct DeviceBridge {
    base: String,
    device: DeviceId,
    names: std::sync::Arc<dyn crate::discovery::FeatureNaming>,
}

impl DeviceBridge {
    /// A bridge for `device` under topic root `base`, with no naming catalogue: enumerations are
    /// published with the appliance's own member names.
    #[must_use]
    pub fn new(base: impl Into<String>, device: DeviceId) -> Self {
        Self {
            base: base.into(),
            device,
            names: std::sync::Arc::new(crate::discovery::DerivedNames),
        }
    }

    /// The same bridge, publishing enumeration values through `names`.
    ///
    /// The catalogue is per device, like `base` and `device`. It is used only for outbound values;
    /// [`handle_set`] takes its catalogue explicitly so that reinterpreting a payload bound for an
    /// appliance is visible at the call site.
    #[must_use]
    pub fn with_names(self, names: std::sync::Arc<dyn crate::discovery::FeatureNaming>) -> Self {
        Self { names, ..self }
    }

    /// The device's `availability` topic.
    #[must_use]
    pub fn availability_topic(&self) -> String {
        format!("{}/{}/availability", self.base, self.device.as_str())
    }

    /// The device's `connection_state` topic.
    #[must_use]
    pub fn connection_state_topic(&self) -> String {
        format!("{}/{}/connection_state", self.base, self.device.as_str())
    }

    /// The full topic for a device-relative path: `<base>/<device>/<relative>`.
    #[must_use]
    pub fn device_topic(&self, relative: &str) -> String {
        format!("{}/{}/{relative}", self.base, self.device.as_str())
    }

    /// The broker will (LWT) that turns this device `offline` on an unclean disconnect.
    #[must_use]
    pub fn last_will(&self) -> LastWill {
        LastWill {
            topic: self.availability_topic(),
            payload: AVAILABILITY_OFFLINE.to_vec(),
            retain: true,
        }
    }

    /// Publishes the device discovery message for `announcement`.
    ///
    /// Retiring the per-entity configs earlier versions published needs the broker's retained
    /// messages, so it is the caller's ([`crate::discovery::is_legacy_config`]).
    ///
    /// # Errors
    ///
    /// [`MqttError`] if the publish fails.
    pub async fn publish_discovery<C: MqttClient>(
        &self,
        client: &mut C,
        mapping: &DeviceMapping,
        announcement: &Announcement<'_>,
    ) -> Result<(), MqttError> {
        let discovery = device_discovery(&self.base, &self.device, mapping, announcement);
        let payload = serde_json::to_vec(&discovery.payload)
            .map_err(|error| MqttError::Client(error.to_string()))?;
        client
            .publish(&discovery.config_topic, &payload, true)
            .await
    }

    /// Publishes one appliance value on its feature's `…/state` topic, **retained**.
    ///
    /// Retained so that a subscriber arriving later learns the current value.
    ///
    /// The value is rendered by [`rendered_value`] and then given the same label the announced
    /// `options` use, because HA discards a state that is not one of its options. A reported `null`,
    /// an unresolved program pointer, or any state outside the announced options is published as
    /// [`PAYLOAD_NONE`] so HA shows *unknown* rather than a stale value. An update that carried no
    /// value, only a description revision, publishes nothing: the entity keeps its state. An update
    /// whose uid has no `state_topic` in the mapping is dropped rather than published to a guessed
    /// topic. Neither the topic nor the payload is logged.
    ///
    /// # Errors
    ///
    /// [`MqttError`] if the publish fails.
    pub async fn publish_value<C: MqttClient>(
        &self,
        client: &mut C,
        mapping: &DeviceMapping,
        model: &EntityModel,
        update: &hc_proto::ValueUpdate,
    ) -> Result<(), MqttError> {
        let Some(entry) = mapping
            .entries()
            .iter()
            .find(|entry| u64::from(entry.uid) == update.uid)
        else {
            return Ok(());
        };
        let Some(relative) = entry.state_topic.as_deref() else {
            return Ok(());
        };
        let Some(value) = &update.value else {
            return Ok(());
        };
        let rendered = rendered_value(model, entry.uid, value);
        // Relabel with the same function and catalogue the announce used for `options`. This
        // happens only at the MQTT edge: REST and SSE keep the appliance's own member name so a
        // browser can write it back.
        let presented = crate::presentation::present(entry, self.names.as_ref());
        let labelled = presented.member_label(&rendered);
        // `None` for: a reported `null`; a program pointer naming no declared program (uid 0 when
        // idle); any state the announced `options` do not contain.
        let payload = if value.is_null()
            || is_unresolved_program(model, entry.uid, value)
            || unannounced_option(entry, &presented, labelled)
        {
            PAYLOAD_NONE
        } else {
            labelled.as_bytes()
        };
        client
            .publish(&self.device_topic(relative), payload, true)
            .await
    }

    /// Subscribes every writable feature's `…/set` topic, the synthetic control topics and
    /// [`HA_STATUS_TOPIC`].
    ///
    /// # Errors
    ///
    /// [`MqttError`] if a subscribe fails.
    pub async fn subscribe_command_topics<C: MqttClient>(
        &self,
        client: &mut C,
        mapping: &DeviceMapping,
    ) -> Result<(), MqttError> {
        // An arriving message is routed through the guard by `handle_set`.
        for relative in mapping.command_topics() {
            let topic = self.device_topic(relative);
            client.subscribe(&topic).await?;
        }
        // The synthetic controls back no feature, so `command_topics()` does not list them.
        for relative in crate::discovery::synthetic_control_topics(mapping) {
            client.subscribe(&self.device_topic(relative)).await?;
        }
        // HA's birth topic. Each device has its own connection, so each subscribes and re-announces
        // itself; the caller routes it via `is_home_assistant_birth`.
        client.subscribe(HA_STATUS_TOPIC).await?;
        Ok(())
    }

    /// Announces the device: discovery plus the `…/set` subscriptions.
    ///
    /// Nothing is published on the `…/state` topics here: an empty retained payload would erase the
    /// previous run's value, and HA rejects `''` against an entity's `options`. State topics are
    /// published by [`Self::publish_value`] once there is a value.
    ///
    /// # Errors
    ///
    /// [`MqttError`] if any publish or subscribe fails.
    pub async fn announce<C: MqttClient>(
        &self,
        client: &mut C,
        mapping: &DeviceMapping,
        announcement: &Announcement<'_>,
    ) -> Result<(), MqttError> {
        self.publish_discovery(client, mapping, announcement)
            .await?;
        self.subscribe_command_topics(client, mapping).await
    }

    /// Publishes the device's `availability` (online/offline) as a retained message.
    ///
    /// # Errors
    ///
    /// [`MqttError`] if the publish fails.
    pub async fn publish_availability<C: MqttClient>(
        &self,
        client: &mut C,
        online: bool,
    ) -> Result<(), MqttError> {
        let payload = if online {
            AVAILABILITY_ONLINE
        } else {
            AVAILABILITY_OFFLINE
        };
        client
            .publish(&self.availability_topic(), payload, true)
            .await
    }

    /// Publishes the device's `connection_state` from [`ConnectionState`].
    ///
    /// # Errors
    ///
    /// [`MqttError`] if the publish fails.
    pub async fn publish_connection_state<C: MqttClient>(
        &self,
        client: &mut C,
        state: ConnectionState,
    ) -> Result<(), MqttError> {
        client
            .publish(
                &self.connection_state_topic(),
                state.as_str().as_bytes(),
                true,
            )
            .await
    }
}

/// The wire integer for an HA `select` label, resolved through the feature's enumeration.
///
/// Only values in this appliance's `declared_values` match, not the whole `members` catalogue
/// from the `FeatureMapping`, so a value the appliance never declared is never written. The HA
/// `select` options are built from `declared_values` too, so no offered option is refused.
///
/// Returns `None` if `label` is not a declared member name of `enum_type`.
#[must_use]
pub fn enum_value_for_label(enum_type: &EnumType, label: &str) -> Option<i64> {
    enum_type
        .members
        .iter()
        .find(|(value, name)| name.as_str() == label && enum_type.declared_values.contains(*value))
        .map(|(value, _)| *value)
}

/// How one appliance value is rendered for display on MQTT, REST and SSE.
///
/// A program pointer shows its program key, an enumeration its declared member name, anything
/// else the raw JSON.
///
/// Only an ordinal the appliance declares and the mapping names becomes a label; anything else is
/// passed through rather than dropped or given an invented name.
#[must_use]
pub fn rendered_value(model: &EntityModel, uid: u16, value: &Value) -> String {
    // A program pointer holds a reference to a `<program>`, not an enumeration value.
    if let Some(name) = program_name(model, uid, value) {
        return name;
    }
    let label = model
        .entity(uid)
        .filter(|entity| matches!(entity.value_type, ValueType::Enumeration { .. }))
        .and_then(|entity| model.enum_type_of(entity))
        .zip(value.as_i64())
        .and_then(|(enum_type, ordinal)| {
            // Declared, not merely catalogued, as on the write path.
            enum_type
                .declared_values
                .contains(&ordinal)
                .then(|| enum_type.member(ordinal))
                .flatten()
        });
    match (label, value) {
        (Some(label), _) => label.to_owned(),
        // A JSON string is published as its contents, not with the quotes serde would add.
        (None, Value::String(text)) => text.clone(),
        (None, other) => other.to_string(),
    }
}

/// Whether `state` is a value Home Assistant will refuse against the `options` this entity was
/// announced with.
///
/// HA ignores such a state and keeps showing the previous one (for example the hob's zone selector
/// reports `0`, which is not a member), so the caller publishes [`PAYLOAD_NONE`] instead. Whether
/// an entity has options is asked of [`crate::discovery::announces_options`], and the comparison is
/// against the labelled options, because that is the list HA holds.
fn unannounced_option(
    entry: &crate::mapping::EntityMapping,
    presented: &crate::presentation::PresentedEntity,
    state: &str,
) -> bool {
    crate::discovery::announces_options(entry)
        && !entry
            .options
            .iter()
            .any(|member| presented.member_label(member) == state)
}

/// Whether this update is a program pointer that resolves to no declared program.
///
/// Separate from [`program_name`], which also returns [`None`] for entities that are not pointers.
fn is_unresolved_program(model: &EntityModel, uid: u16, value: &Value) -> bool {
    model.entity(uid).is_some_and(|entity| {
        matches!(
            entity.kind,
            hc_profile::EntityKind::ActiveProgram | hc_profile::EntityKind::SelectedProgram
        )
    }) && program_name(model, uid, value).is_none()
}

/// The dotted key of the program a program pointer points at, or [`None`] when this entity is not
/// a pointer or the uid names no declared program.
///
/// `<activeProgram>` and `<selectedProgram>` hold the uid of a `<program>` in the same description.
/// Uid `0` means idle; an undeclared uid means a program outside the exported profile. Both give
/// [`None`]; [`DeviceBridge::publish_value`] maps that to [`PAYLOAD_NONE`] at the MQTT edge, while
/// REST and SSE get the raw uid.
#[must_use]
fn program_name(model: &EntityModel, uid: u16, value: &Value) -> Option<String> {
    let entity = model.entity(uid)?;
    if !matches!(
        entity.kind,
        hc_profile::EntityKind::ActiveProgram | hc_profile::EntityKind::SelectedProgram
    ) {
        return None;
    }
    let target = u16::try_from(value.as_i64()?).ok()?;
    model.programs().get(&target)?.key.clone()
}

/// The wire integer for an enumeration `…/set` payload, in either accepted form.
///
/// Two forms, tried in this order:
///
/// 1. a declared member label, through [`enum_value_for_label`] (the HA `select` form);
/// 2. a declared wire ordinal, the form the appliance reports, so a value just read can be
///    written back.
///
/// Label first, so a member literally named `"0"` keeps its meaning. Both arms check this
/// appliance's `declared_values`; a declared ordinal without a member name is accepted.
///
/// Returns `None` if `payload` is neither a declared label nor a declared ordinal.
#[must_use]
pub fn enum_wire_value(enum_type: &EnumType, payload: &str) -> Option<i64> {
    enum_value_for_label(enum_type, payload).or_else(|| {
        let ordinal: i64 = payload.trim().parse().ok()?;
        enum_type
            .declared_values
            .contains(&ordinal)
            .then_some(ordinal)
    })
}

/// Converts a `…/set` payload to the wire value for a feature.
///
/// An enumeration feature takes either a declared member label or a declared wire ordinal and
/// resolves it to the wire integer ([`enum_wire_value`]); any other feature takes the payload as
/// JSON (a number, bool or string), falling back to a plain string if it is not JSON.
fn value_for_write(model: &EntityModel, entity: &Entity, payload: &str) -> Result<Value, SetError> {
    match entity.value_type {
        ValueType::Enumeration { .. } => {
            let enum_type = model
                .enum_type_of(entity)
                .ok_or_else(|| SetError::EnumLabelUnknown(payload.to_owned()))?;
            let wire = enum_wire_value(enum_type, payload)
                .ok_or_else(|| SetError::EnumLabelUnknown(payload.to_owned()))?;
            Ok(Value::from(wire))
        }
        // Numeric / unconstrained: accept the JSON value, or a bare string if it is not JSON.
        _ => Ok(serde_json::from_str::<Value>(payload)
            .unwrap_or_else(|_| Value::String(payload.to_owned()))),
    }
}

/// Builds the [`WriteCommand`] an incoming `…/set` maps to, **without** transmitting it.
///
/// This is the exact command [`handle_set`] hands to the guard: usually a `/ro/values` write of the
/// feature's uid and the converted value; a program selection or the stop control become their own
/// frames. [`to_outgoing`] turns it into the wire frame.
///
/// # Errors
///
/// [`SetError`] if the topic matches no writable feature, the model lacks the entity, or the
/// payload is not a value the feature accepts.
pub fn build_write_command(
    model: &EntityModel,
    mapping: &DeviceMapping,
    relative_set_topic: &str,
    payload: &str,
) -> Result<WriteCommand, SetError> {
    // The synthetic controls back no feature, so the mapping lookup below would refuse them.
    if relative_set_topic == STOP_PROGRAM_TOPIC {
        return Ok(stop_program_command());
    }
    let entry = mapping
        .entries()
        .iter()
        .find(|entry| entry.command_topic.as_deref() == Some(relative_set_topic))
        .ok_or_else(|| SetError::UnknownTopic(relative_set_topic.to_owned()))?;
    let entity = model
        .entity(entry.uid)
        .ok_or(SetError::UnknownEntity(entry.uid))?;
    // A programme pointer takes a different frame entirely — see `select_program_command`.
    if matches!(entity.kind, hc_profile::EntityKind::SelectedProgram) {
        return select_program_command(model, payload);
    }
    let value = value_for_write(model, entity, payload)?;
    Ok(WriteCommand {
        resource: VALUES_RESOURCE.to_owned(),
        target_uid: Some(entry.uid),
        field: None,
        value,
        validate: true,
        options: Vec::new(),
    })
}

/// The command the synthetic stop control becomes: `DELETE /ro/activeProgram`, no body, no uid.
///
/// This aborts whatever the appliance is running. The payload is ignored: a button press has only
/// one meaning.
fn stop_program_command() -> WriteCommand {
    WriteCommand {
        resource: ACTIVE_PROGRAM_RESOURCE.to_owned(),
        // No uid: the resource is the whole address, so this is its own `WriteIntent`.
        target_uid: None,
        field: None,
        value: Value::Null,
        validate: false,
        options: Vec::new(),
    }
}

/// The command a programme selection becomes: a program-start frame on `/ro/selectedProgram`.
///
/// This starts a programme on the appliance; on a cooktop or oven that is a physical act. The
/// frame addresses the programme by uid, not the pointer feature, since there is no known
/// `/ro/values` form for a selection. The payload must name a programme this appliance declares
/// (the `select` options come from the same list), so a typo is an error rather than a start.
/// `options` is left empty: a caller that knows a programme's options builds the command directly.
fn select_program_command(model: &EntityModel, payload: &str) -> Result<WriteCommand, SetError> {
    let wanted = payload.trim();
    let program = model
        .programs()
        .values()
        .find(|program| program.key.as_deref() == Some(wanted))
        .ok_or_else(|| {
            SetError::InvalidPayload(format!(
                "'{wanted}' is not a programme this appliance declares"
            ))
        })?;
    Ok(WriteCommand {
        resource: SELECTED_PROGRAM_RESOURCE.to_owned(),
        // The programme's uid, emitted as the frame's `program` field.
        target_uid: Some(program.uid),
        field: None,
        // For the inspector only; not a wire field on this path.
        value: Value::from(program.uid),
        validate: false,
        options: Vec::new(),
    })
}

/// Routes an incoming `…/set` message to a device write through the guard.
///
/// Resolves `relative_set_topic` to a writable feature, converts `payload` (see
/// [`enum_wire_value`] for enumerations) and calls [`GuardedWriter::write`], the only path to the
/// transport. This sends a real command to the appliance.
///
/// # Errors
///
/// [`SetError`] if the topic is unknown, the payload is invalid, or the write fails.
pub async fn handle_set<T: WriteTransport>(
    writer: &mut GuardedWriter<T>,
    device: &SetContext<'_>,
    relative_set_topic: &str,
    payload: &str,
) -> Result<(), SetError> {
    // HA sends back the label it was announced, so a contributed member name is mapped back to
    // the appliance's own member first. A payload that is already a declared member is never
    // reinterpreted (see `PresentedEntity::raw_member`).
    let resolved =
        resolve_member_payload(device.mapping, relative_set_topic, payload, device.names);
    let command = build_write_command(device.model, device.mapping, relative_set_topic, &resolved)?;
    // The only path to the transport: it is private to the guard.
    writer.write(&command).await.map_err(SetError::from)
}

/// The per-device tables an inbound `…/set` is resolved against.
///
/// All per device and read-only. Separate from [`crate::discovery::Announcement`], whose discovery
/// mode and device info have no place on the write path.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct SetContext<'a> {
    /// The appliance's parsed description — enumerations, kinds, declared values.
    pub model: &'a EntityModel,
    /// The feature→topic mapping the `…/set` topic is looked up in.
    pub mapping: &'a DeviceMapping,
    /// The naming catalogue, so a contributed member name Home Assistant was given is understood
    /// coming back. [`crate::discovery::DerivedNames`] for a caller with none.
    pub names: &'a dyn crate::discovery::FeatureNaming,
}

impl<'a> SetContext<'a> {
    /// A context over `model` and `mapping` with no naming catalogue.
    #[must_use]
    pub fn new(model: &'a EntityModel, mapping: &'a DeviceMapping) -> Self {
        Self {
            model,
            mapping,
            names: &crate::discovery::DerivedNames,
        }
    }

    /// The same context, understanding the member names `names` supplies.
    #[must_use]
    pub const fn with_names(self, names: &'a dyn crate::discovery::FeatureNaming) -> Self {
        Self { names, ..self }
    }
}

/// Turns a contributed member name back into the appliance's own, when that is what arrived.
///
/// Returns the payload unchanged otherwise: non-enumeration features, bare ordinals and members
/// with no contributed name.
fn resolve_member_payload<'p>(
    mapping: &DeviceMapping,
    relative_set_topic: &str,
    payload: &'p str,
    names: &dyn crate::discovery::FeatureNaming,
) -> std::borrow::Cow<'p, str> {
    let Some(entry) = mapping
        .entries()
        .iter()
        .find(|entry| entry.command_topic.as_deref() == Some(relative_set_topic))
    else {
        // An unknown topic or a synthetic control; `build_write_command` handles both.
        return std::borrow::Cow::Borrowed(payload);
    };
    if entry.options.is_empty() {
        return std::borrow::Cow::Borrowed(payload);
    }
    let presented = crate::presentation::present(entry, names);
    presented
        .raw_member(&entry.options, payload)
        .map_or(std::borrow::Cow::Borrowed(payload), |member| {
            std::borrow::Cow::Owned(member.to_owned())
        })
}

/// Why a [`WriteCommand`] could not be turned into a wire frame.
///
/// Every variant means no bytes were emitted: [`to_outgoing`] runs before
/// [`hc_transport::Sender::send`].
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum WriteFrameError {
    /// A resource with no known frame; everything outside the implemented resources is refused.
    #[error(
        "no write frame is implemented for resource '{0}' (/ro/values and /ro/selectedProgram are)"
    )]
    UnsupportedResource(String),
    /// A write with no `target_uid`; features and programs are addressed by `uid` only.
    #[error("a write needs a target uid")]
    MissingTargetUid,
    /// The value has no wire encoding (`null`, an array or an object).
    #[error("the value has no wire encoding: {0}")]
    UnrepresentableValue(String),
    /// `hc-proto` refused to build the frame (e.g. an empty item list).
    #[error("hc-proto refused the write frame: {0}")]
    Frame(String),
}

/// The per-connection handshake state a write frame needs, which `hc_transport::Sender` does not
/// carry.
///
/// - `s_id`: the `sID` of the device's opening `POST /ei/initialValues`
///   (`hc_proto::Handshake::s_id`);
/// - `msg_ids`: the connection's own [`hc_proto::MsgIdAllocator`] (`hc_proto::Handshake::msg_ids`),
///   shared with the session so that writes and session frames never reuse an id;
/// - `ro_version`: the `ro` entry of the `/ci/services` table, default `Version::Int(1)`
///   (`hc_proto::Handshake::version_for`).
#[derive(Debug, Clone)]
pub struct WriteSessionState {
    s_id: u64,
    msg_ids: MsgIdAllocator,
    ro_version: Version,
    acks: Option<WriteAcks>,
}

impl WriteSessionState {
    /// The session state for one connection's writes.
    ///
    /// `msg_ids` must be the session's own allocator handle (`hc_proto::Handshake::msg_ids`).
    #[must_use]
    pub const fn new(s_id: u64, msg_ids: MsgIdAllocator, ro_version: Version) -> Self {
        Self {
            s_id,
            msg_ids,
            ro_version,
            acks: None,
        }
    }

    /// Makes each write wait for the appliance's answer, delivered through `acks`.
    #[must_use]
    pub fn with_acks(mut self, acks: WriteAcks) -> Self {
        self.acks = Some(acks);
        self
    }

    /// Spends one `msgID` from the connection's shared allocator. Callers validate first, so a
    /// refused write spends no id; taking rather than peeking means the session cannot hand out the
    /// same id in between.
    fn take_msg_id(&self) -> u64 {
        self.msg_ids.take()
    }
}

/// The real-transport [`WriteTransport`] adapter.
///
/// It owns an [`hc_transport::Sender`] in a private field with no accessor, so the only way to
/// send bytes is [`WriteTransport::transmit`] via [`GuardedWriter::write`]. Its constructor is
/// `pub(crate)`; callers obtain one sealed in a guard from [`guarded_session_writer`].
pub struct SessionWriter<Si, E> {
    sender: Sender<Si, E>,
    session: WriteSessionState,
}

impl<Si, E> core::fmt::Debug for SessionWriter<Si, E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The sender holds the TX chain and session state; nothing here is safe to show.
        f.debug_struct("SessionWriter").finish_non_exhaustive()
    }
}

impl<Si, E> SessionWriter<Si, E> {
    /// Wraps a [`Sender`], consuming it; the caller must retain no other clone.
    ///
    /// `pub(crate)` because a `SessionWriter` outside a guard would be a second write path that
    /// bypasses [`GuardedWriter`].
    #[must_use]
    pub(crate) const fn new(sender: Sender<Si, E>, session: WriteSessionState) -> Self {
        Self { sender, session }
    }
}

/// Converts a [`WriteCommand`] into the exact frame that goes on the wire.
///
/// That is a `POST /ro/values` with one `{"uid":…,"value":…}` item, a program start on
/// `/ro/selectedProgram`, or a `DELETE /ro/activeProgram`.
///
/// Building a frame does not send it. `command.field` and `command.validate` are not wire fields
/// and are ignored: the wire addresses a feature by `uid` alone.
///
/// # Errors
///
/// [`WriteFrameError`]:
///
/// - [`WriteFrameError::UnsupportedResource`] for any other resource;
/// - [`WriteFrameError::MissingTargetUid`] when there is no uid to address;
/// - [`WriteFrameError::UnrepresentableValue`] / [`WriteFrameError::Frame`] when `hc-proto` refuses
///   the value or the frame.
pub fn to_outgoing(
    command: &WriteCommand,
    s_id: u64,
    msg_id: u64,
    version: Version,
) -> Result<OutgoingMessage, WriteFrameError> {
    frame_for_intent(validate_write(command)?, s_id, msg_id, version)
}

/// Every refusal [`to_outgoing`] can make, decided before a `msgID` is taken, so a refused write
/// spends none.
fn validate_write(command: &WriteCommand) -> Result<WriteIntent, WriteFrameError> {
    match command.resource.as_str() {
        VALUES_RESOURCE => Ok(WriteIntent::Feature(write_item(
            command.target_uid,
            &command.value,
        )?)),
        SELECTED_PROGRAM_RESOURCE => {
            let program = command
                .target_uid
                .ok_or(WriteFrameError::MissingTargetUid)?;
            let mut options = Vec::with_capacity(command.options.len());
            for (uid, value) in &command.options {
                options.push(write_item(Some(*uid), value)?);
            }
            Ok(WriteIntent::Program {
                program: u32::from(program),
                options,
            })
        }
        // The stop control: no items and no uid, so nothing to validate.
        ACTIVE_PROGRAM_RESOURCE => Ok(WriteIntent::StopProgram),
        // Default-deny: a resource with no known frame is refused rather than guessed at.
        other => Err(WriteFrameError::UnsupportedResource(other.to_owned())),
    }
}

/// One `{"uid","value"}` item from a uid and a JSON value.
fn write_item(uid: Option<u16>, value: &Value) -> Result<WriteItem, WriteFrameError> {
    let uid = uid.ok_or(WriteFrameError::MissingTargetUid)?;
    WriteItem::from_json(u32::from(uid), value)
        .map_err(|error| WriteFrameError::UnrepresentableValue(error.to_string()))
}

/// What a [`WriteCommand`] resolved to, decided before a `msgID` is spent.
#[derive(Debug)]
enum WriteIntent {
    /// A `/ro/values` feature write: one item.
    Feature(WriteItem),
    /// A `/ro/selectedProgram` program start: a program uid plus its options.
    Program {
        /// The program uid.
        program: u32,
        /// Its options, in wire order.
        options: Vec<WriteItem>,
    },
    /// A `/ro/activeProgram` abort: a `DELETE` with no body and no uid. A separate variant keeps
    /// the uid requirement unconditional for [`Self::Feature`].
    StopProgram,
}

/// The one write-frame builder, shared by [`to_outgoing`] and [`SessionWriter`] so their bytes
/// cannot diverge.
fn frame_for_intent(
    intent: WriteIntent,
    s_id: u64,
    msg_id: u64,
    version: Version,
) -> Result<OutgoingMessage, WriteFrameError> {
    match intent {
        // `values_write` refuses an empty item list, which cannot happen with one item; the
        // `Result` is threaded rather than unwrapped.
        WriteIntent::Feature(item) => {
            OutgoingMessage::values_write(s_id, msg_id, version, vec![item])
                .map_err(|error| WriteFrameError::Frame(error.to_string()))
        }
        // Infallible: an empty options list is legal and emits `"options":[]` (`hc_proto::Body::Program`).
        WriteIntent::Program { program, options } => Ok(OutgoingMessage::program_start(
            s_id, msg_id, version, program, options,
        )),
        // `bare` emits `Body::Empty`, which omits the `data` key rather than sending `null` or `[]`.
        WriteIntent::StopProgram => Ok(OutgoingMessage::bare(
            s_id,
            msg_id,
            ACTIVE_PROGRAM_RESOURCE,
            version,
            hc_proto::Action::Delete,
        )),
    }
}

#[async_trait]
impl<Si: FrameSink, E: FrameEncoder> WriteTransport for SessionWriter<Si, E> {
    async fn transmit(&mut self, command: &WriteCommand) -> Result<(), String> {
        // Validate, then take, then build: a refused write spends no id, and taking (not peeking)
        // from the shared allocator means the session cannot hand out the same id meanwhile.
        let intent = validate_write(command).map_err(|error| error.to_string())?;
        let msg_id = self.session.take_msg_id();
        let message = frame_for_intent(
            intent,
            self.session.s_id,
            msg_id,
            self.session.ro_version.clone(),
        )
        .map_err(|error| error.to_string())?;
        let Some(acks) = self.session.acks.clone() else {
            return self
                .sender
                .send(&message)
                .await
                .map_err(|error| error.to_string());
        };
        // Registered before sending, so an answer that arrives at once is not missed.
        let answer = acks.register(msg_id);
        if let Err(error) = self.sender.send(&message).await {
            acks.forget(msg_id);
            return Err(error.to_string());
        }
        match tokio::time::timeout(crate::acks::ACK_TIMEOUT, answer).await {
            Ok(Ok(verdict)) => verdict,
            Ok(Err(_closed)) => {
                Err("the connection closed before the appliance answered".to_owned())
            }
            Err(_elapsed) => {
                acks.forget(msg_id);
                tracing::debug!(
                    msg_id,
                    "no answer to a write; assuming the appliance took it"
                );
                Ok(())
            }
        }
    }
}

/// Seals a real [`Sender`] inside a [`GuardedWriter`] with no separately-callable handle to it.
///
/// After this returns, bytes can only be sent via [`GuardedWriter::write`]. `session` is the
/// per-connection state a write frame needs; see [`WriteSessionState`].
#[must_use]
pub const fn guarded_session_writer<Si, E>(
    sender: Sender<Si, E>,
    session: WriteSessionState,
) -> GuardedWriter<SessionWriter<Si, E>>
where
    Si: FrameSink,
    E: FrameEncoder,
{
    GuardedWriter::new(SessionWriter::new(sender, session))
}

/// Takes an `hc_transport::Session`'s one send handle and yields the guarded writer.
///
/// The handle can be taken only once per session, the `Sender` never surfaces, and `sID`, the
/// `msgID` allocator and the `ro` version all come from the session's own handshake. The session
/// is borrowed so it can keep running `run_steady_state`; use [`guarded_session`] when the caller
/// has no further use for it.
///
/// Returns `None` if this session already yielded its handle.
#[must_use]
pub fn take_guarded_session<Si, St, E, D>(
    session: &mut Session<Si, St, E, D>,
) -> Option<GuardedWriter<SessionWriter<Si, E>>>
where
    Si: FrameSink,
    St: FrameStream,
    E: FrameEncoder,
    D: FrameDecoder,
{
    session.take_guarded(|sender, s_id, msg_ids, ro_version| {
        guarded_session_writer(sender, WriteSessionState::new(s_id, msg_ids, ro_version))
    })
}

/// Consumes an `hc_transport::Session` and yields the guarded writer: [`take_guarded_session`]
/// for a caller with no further use for the session.
///
/// Returns `None` if this session already yielded its handle.
#[must_use]
pub fn guarded_session<Si, St, E, D>(
    session: Session<Si, St, E, D>,
) -> Option<GuardedWriter<SessionWriter<Si, E>>>
where
    Si: FrameSink,
    St: FrameStream,
    E: FrameEncoder,
    D: FrameDecoder,
{
    session.into_guarded(|sender, s_id, msg_ids, ro_version| {
        guarded_session_writer(sender, WriteSessionState::new(s_id, msg_ids, ro_version))
    })
}

/// A type-erased [`WriteTransport`], for a per-device writer registry.
///
/// The AES and TLS-PSK transports are different types, so a registry cannot name
/// `GuardedWriter<SessionWriter<Si, E>>`. The erasure happens inside this crate, under the guard.
/// `Send` because the registry is shared across tasks.
pub type BoxedTransport = Box<dyn WriteTransport + Send>;

/// Delegation, so a boxed transport can sit inside a [`GuardedWriter`]. The box is only built by
/// [`take_guarded_session_boxed`] and [`guarded_session_boxed`], which put it straight into a guard.
#[async_trait]
impl WriteTransport for BoxedTransport {
    async fn transmit(&mut self, command: &WriteCommand) -> Result<(), String> {
        // `(**self)` is the trait object, so this is a virtual call, not recursion.
        (**self).transmit(command).await
    }
}

/// The one place a [`SessionWriter`] is boxed.
///
/// `tests/write_choke_point.rs` counts the constructor's call sites as text, so do not spell the
/// call out in comments here. Private: a bare [`BoxedTransport`] bypasses the guard, and every
/// caller puts it straight into a [`GuardedWriter`].
fn boxed_session_writer<Si, E>(sender: Sender<Si, E>, session: WriteSessionState) -> BoxedTransport
where
    Si: FrameSink + Send + 'static,
    E: FrameEncoder + Send + 'static,
{
    Box::new(SessionWriter::new(sender, session))
}

/// [`take_guarded_session`], type-erased: one guard around one boxed [`SessionWriter`].
///
/// Boxing happens under the guard, never around it, so [`GuardedWriter::write`] stays the only
/// route to [`WriteTransport::transmit`] and the caller still gets a typed [`WriteError`]. The
/// session is borrowed so it can keep running `run_steady_state`.
///
/// Returns `None` if this session already yielded its handle.
#[must_use]
pub fn take_guarded_session_boxed<Si, St, E, D>(
    session: &mut Session<Si, St, E, D>,
) -> Option<GuardedWriter<BoxedTransport>>
where
    Si: FrameSink + Send + 'static,
    St: FrameStream,
    E: FrameEncoder + Send + 'static,
    D: FrameDecoder,
{
    session.take_guarded(|sender, s_id, msg_ids, ro_version| {
        let transport =
            boxed_session_writer(sender, WriteSessionState::new(s_id, msg_ids, ro_version));
        GuardedWriter::new(transport)
    })
}

/// [`take_guarded_session_boxed`], with each write waiting for the appliance's answer.
///
/// The caller feeds every received message to `acks` ([`WriteAcks::resolve`]) and closes it when
/// the connection ends, so a write returns the appliance's verdict rather than just "sent".
#[must_use]
pub fn take_guarded_session_acked<Si, St, E, D>(
    session: &mut Session<Si, St, E, D>,
    acks: WriteAcks,
) -> Option<GuardedWriter<BoxedTransport>>
where
    Si: FrameSink + Send + 'static,
    St: FrameStream,
    E: FrameEncoder + Send + 'static,
    D: FrameDecoder,
{
    session.take_guarded(|sender, s_id, msg_ids, ro_version| {
        let state = WriteSessionState::new(s_id, msg_ids, ro_version).with_acks(acks);
        GuardedWriter::new(boxed_session_writer(sender, state))
    })
}

/// [`guarded_session`], type-erased: the consuming twin of [`take_guarded_session_boxed`].
#[must_use]
pub fn guarded_session_boxed<Si, St, E, D>(
    session: Session<Si, St, E, D>,
) -> Option<GuardedWriter<BoxedTransport>>
where
    Si: FrameSink + Send + 'static,
    St: FrameStream,
    E: FrameEncoder + Send + 'static,
    D: FrameDecoder,
{
    session.into_guarded(|sender, s_id, msg_ids, ro_version| {
        let transport =
            boxed_session_writer(sender, WriteSessionState::new(s_id, msg_ids, ro_version));
        GuardedWriter::new(transport)
    })
}
