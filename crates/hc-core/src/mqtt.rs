//! The MQTT client seam.
//!
//! The publish / subscribe surface the bridge ([`crate::bridge`]) needs is the [`MqttClient`]
//! trait, so tests can use an in-process fake; [`RumqttcClient`] is the production implementation.
//! This module never builds a topic: the bridge builds every topic from a [`crate::DeviceId`]
//! slug, and topics never carry appliance identifiers.

use async_trait::async_trait;
use thiserror::Error;

/// The broker will (LWT): what the broker publishes when the connection drops without a clean
/// disconnect, turning a device `offline`.
///
/// It is connect-time configuration (set on `MqttOptions` before the client exists), so it is a
/// value the client constructor consumes rather than a method on [`MqttClient`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastWill {
    /// The topic the will is published to — the device's `availability` topic.
    pub topic: String,
    /// The retained will payload — `offline`.
    pub payload: Vec<u8>,
    /// Whether the will is retained (it is: a late subscriber must still learn the device is gone).
    pub retain: bool,
}

/// Anything the MQTT client surface can refuse.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum MqttError {
    /// The underlying client failed to publish or subscribe.
    #[error("mqtt client error: {0}")]
    Client(String),
}

/// The publish / subscribe surface the bridge drives.
///
/// The `QoS` is the implementation's choice.
#[async_trait]
pub trait MqttClient: Send {
    /// Publishes `payload` to `topic`, optionally retained.
    ///
    /// # Errors
    ///
    /// [`MqttError::Client`] if the underlying client rejects the publish.
    async fn publish(&mut self, topic: &str, payload: &[u8], retain: bool)
    -> Result<(), MqttError>;

    /// Subscribes to `topic_filter` (a `…/set` topic, in this crate).
    ///
    /// # Errors
    ///
    /// [`MqttError::Client`] if the underlying client rejects the subscription.
    async fn subscribe(&mut self, topic_filter: &str) -> Result<(), MqttError>;
}

/// The production [`MqttClient`], a thin wrapper over [`rumqttc::AsyncClient`].
///
/// The caller builds the `MqttOptions` (including the [`LastWill`] via
/// [`rumqttc::MqttOptions::set_last_will`]) and drives the `EventLoop`; this wrapper only forwards
/// publish / subscribe.
#[derive(Debug)]
pub struct RumqttcClient {
    client: rumqttc::AsyncClient,
}

impl RumqttcClient {
    /// Wraps an already-constructed [`rumqttc::AsyncClient`].
    #[must_use]
    pub const fn new(client: rumqttc::AsyncClient) -> Self {
        Self { client }
    }
}

/// The `QoS` a retained publish goes out at: `AtMostOnce`.
///
/// A retained topic already has the durability `QoS` 1 would add: the broker replays the last
/// payload to every later subscriber, and a lost publish is replaced by the next one. `QoS` 0 also
/// takes no packet id. An announce publishes hundreds of retained configs in a tight loop, which at
/// `QoS` 1 wraps rumqttc's in-flight id space; each packet-id collision stalls the outgoing
/// pipeline and can end in a disconnect. Subscriptions and the last will stay at `AtLeastOnce`,
/// because nothing repairs a lost `SUBSCRIBE`.
const RETAINED_QOS: rumqttc::QoS = rumqttc::QoS::AtMostOnce;

/// The `QoS` an un-retained publish goes out at.
///
/// Nothing publishes un-retained today. Kept distinct from [`RETAINED_QOS`] so a future un-retained
/// publish does not inherit a `QoS` chosen for retained topics.
const UNRETAINED_QOS: rumqttc::QoS = rumqttc::QoS::AtLeastOnce;

#[async_trait]
impl MqttClient for RumqttcClient {
    async fn publish(
        &mut self,
        topic: &str,
        payload: &[u8],
        retain: bool,
    ) -> Result<(), MqttError> {
        let qos = if retain { RETAINED_QOS } else { UNRETAINED_QOS };
        self.client
            .publish(topic, qos, retain, payload.to_vec())
            .await
            .map_err(|error| MqttError::Client(error.to_string()))
    }

    async fn subscribe(&mut self, topic_filter: &str) -> Result<(), MqttError> {
        self.client
            .subscribe(topic_filter, rumqttc::QoS::AtLeastOnce)
            .await
            .map_err(|error| MqttError::Client(error.to_string()))
    }
}
