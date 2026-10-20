//! The per-device bridge task.
//!
//! Two tasks per device share one broker connection:
//!
//! - the **poller** drives `rumqttc`'s event loop and does nothing else, so keep-alives keep
//!   flowing and the client's request queue keeps draining however long a publish or a write takes;
//! - the **worker** owns every publish. It turns device events and broker messages into publishes,
//!   one at a time, and spawns `…/set` handling so a slow appliance never holds up state updates.
//!
//! Discovery is one retained device message per appliance. It is published only when it changes,
//! and a component that disappears is first retracted on its own, which is how Home Assistant is
//! told to remove it.
//!
//! Earlier versions published one retained config per entity. On each broker connection the worker
//! subscribes to those topics, waits for the retained ones to arrive, and migrates each one it
//! finds: `migrate_discovery`, then the device message, then an empty retained payload. Home
//! Assistant keeps the entity's registry entry throughout, so names, areas and automations survive.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use hc_core::bridge::DeviceBridge;
use hc_core::device::ConnectionState;
use hc_core::discovery::{self, DeviceDiscovery};
use hc_core::mapping::DeviceId;
use hc_core::mqtt::{MqttClient, MqttError, RumqttcClient};
use rumqttc::{Event, EventLoop, MqttOptions, Packet, QoS};
use tokio::sync::mpsc;

use super::{CatalogueNames, DeviceEvent, DeviceEventStream, MqttSettings, SetRouting};

/// Requests the client can queue before a publish waits for the event loop.
const REQUEST_CAPACITY: usize = 1024;

/// Keep-alive interval on the broker connection.
const KEEP_ALIVE: Duration = Duration::from_secs(30);

/// Pause after a broker connection error before polling again; `rumqttc` reconnects on poll.
const BROKER_RETRY: Duration = Duration::from_secs(5);

/// The largest packet either way. A device message carries every component, and its echo comes
/// back on the same connection; `rumqttc`'s default of 10 KiB would drop the connection.
const MAX_PACKET: usize = 1024 * 1024;

/// How long the retained messages may take to arrive before discovery goes ahead without them.
///
/// Normally the scan settles first; this covers a broker whose ACL refuses the sync marker.
const SCAN_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the scan waits after the sync marker without another retained message arriving.
///
/// Some brokers (aedes) send retained messages after a live one published later, so the marker
/// alone does not prove they have all arrived.
const SCAN_SETTLE: Duration = Duration::from_millis(300);

/// A Home Assistant birth message this soon after a full resync is ignored. A retained birth
/// message arrives right after every connect, and the resync it asks for has just happened.
const RESYNC_DEBOUNCE: Duration = Duration::from_secs(30);

/// What the poller hands the worker.
#[derive(Debug)]
enum BrokerEvent {
    /// The broker accepted the connection (first connect or reconnect). Subscriptions and
    /// availability have to be re-established.
    Connected,
    /// The broker connection failed.
    Lost,
    /// Home Assistant published `online` on its status topic: it restarted and wants discovery.
    HomeAssistantOnline,
    /// The device message the broker retained from the last run.
    RetainedDiscovery(Vec<u8>),
    /// A per-entity config an earlier version published.
    Legacy(String),
    /// The sync marker came back.
    Synced { session: u64 },
    /// Whether anything arrived since the last check; `seen` is the count back then.
    ScanCheck { session: u64, seen: u64 },
    /// The scan ran out of time.
    ScanTimeout { session: u64 },
    /// Per-entity configs that arrived after the scan, gathered for one migration.
    LateMigrate,
    /// A message on one of the `…/set` topics.
    Set { topic: String, payload: String },
    /// A `…/set` write finished; carries the device-relative command topic and the outcome.
    SetDone {
        relative: String,
        outcome: SetRouting,
    },
}

/// Spawns the MQTT bridge for one device: its own connection, last will and client id.
pub fn spawn_device_bridge(
    state: crate::state::AppState,
    settings: MqttSettings,
    slug: String,
    events: DeviceEventStream,
) {
    let names = CatalogueNames::for_slug(&state, &slug);
    let bridge = DeviceBridge::new(
        settings.base.clone(),
        hc_core::mapping::DeviceId::new(&slug),
    )
    .with_names(std::sync::Arc::new(names));
    let (client, eventloop) =
        rumqttc::AsyncClient::new(options(&settings, &slug, &bridge), REQUEST_CAPACITY);
    let (broker_tx, broker_rx) = mpsc::unbounded_channel();
    let routes = Routes::new(&settings.base, &slug);

    tokio::spawn(poll(eventloop, broker_tx.clone(), routes.clone()));
    let worker = Worker {
        state,
        settings,
        slug,
        bridge,
        client: Subscriptions::new(RumqttcClient::new(client)),
        broker_tx,
        routes,
        published: None,
        scan: Scan::default(),
        values: BTreeMap::new(),
        appliance: ConnectionState::Offline,
        last_resync: None,
    };
    tokio::spawn(worker.run(events, broker_rx));
}

fn options(settings: &MqttSettings, slug: &str, bridge: &DeviceBridge) -> MqttOptions {
    // The client id is derived from the slug: unique per device, and never an appliance identifier.
    let mut options = MqttOptions::new(
        format!("kessel-{slug}"),
        settings.host.clone(),
        settings.port,
    );
    options.set_keep_alive(KEEP_ALIVE);
    options.set_max_packet_size(MAX_PACKET, MAX_PACKET);
    let will = bridge.last_will();
    options.set_last_will(rumqttc::LastWill::new(
        will.topic,
        will.payload,
        QoS::AtLeastOnce,
        will.retain,
    ));
    if let Some(username) = settings.username.clone() {
        let password = settings
            .password
            .as_ref()
            .map(|password| password.expose_secret().clone())
            .unwrap_or_default();
        options.set_credentials(username, password);
    }
    options
}

/// The topics the poller tells apart, all derived from the slug.
#[derive(Debug, Clone)]
struct Routes {
    device: DeviceId,
    /// This device's discovery message.
    discovery: String,
    /// The marker that ends the retained-message scan; see [`Worker::start_scan`].
    sync: String,
}

impl Routes {
    fn new(base: &str, slug: &str) -> Self {
        let device = DeviceId::new(slug);
        Self {
            discovery: discovery::device_config_topic(&device),
            sync: format!("{base}/{slug}/_control/discovery_sync"),
            device,
        }
    }

    /// What an inbound message means, or [`None`] for one that needs nothing: an echo of the
    /// worker's own publishes, or a clear.
    fn classify(&self, topic: &str, payload: &[u8], retained: bool) -> Option<BrokerEvent> {
        if hc_core::bridge::is_home_assistant_birth(topic, payload) {
            return Some(BrokerEvent::HomeAssistantOnline);
        }
        if topic == self.sync {
            let session = std::str::from_utf8(payload).ok()?.parse().ok()?;
            return Some(BrokerEvent::Synced { session });
        }
        if topic == self.discovery {
            // Only the copy the broker kept; a live one is the worker's own publish coming back.
            return (retained && !payload.is_empty())
                .then(|| BrokerEvent::RetainedDiscovery(payload.to_vec()));
        }
        if discovery::is_legacy_config(&self.device, topic, payload) {
            return Some(BrokerEvent::Legacy(topic.to_owned()));
        }
        if topic.starts_with("homeassistant/") {
            return None;
        }
        Some(BrokerEvent::Set {
            topic: topic.to_owned(),
            payload: String::from_utf8_lossy(payload).into_owned(),
        })
    }
}

/// Drives the connection forever. Never awaits anything but the event loop itself.
async fn poll(
    mut eventloop: EventLoop,
    worker: mpsc::UnboundedSender<BrokerEvent>,
    routes: Routes,
) {
    let slug = routes.device.as_str().to_owned();
    let mut healthy = true;
    loop {
        let event = match eventloop.poll().await {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                if !healthy {
                    tracing::info!(device = %slug, "MQTT broker connection restored");
                }
                healthy = true;
                BrokerEvent::Connected
            }
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                match routes.classify(&publish.topic, &publish.payload, publish.retain) {
                    Some(event) => event,
                    None => continue,
                }
            }
            Ok(_) => continue,
            Err(error) => {
                // Logged once per outage rather than on every retry.
                if healthy {
                    tracing::warn!(device = %slug, %error, "MQTT broker connection lost; retrying");
                }
                healthy = false;
                let _closed = worker.send(BrokerEvent::Lost);
                tokio::time::sleep(BROKER_RETRY).await;
                continue;
            }
        };
        if worker.send(event).is_err() {
            return;
        }
    }
}

/// The device message as last published, or as the broker retained it.
#[derive(Debug)]
struct Published {
    payload: Vec<u8>,
    platforms: BTreeMap<String, String>,
}

impl Published {
    fn from_retained(payload: Vec<u8>) -> Option<Self> {
        let parsed: serde_json::Value = serde_json::from_slice(&payload).ok()?;
        Some(Self {
            platforms: discovery::component_platforms(&parsed),
            payload,
        })
    }
}

/// The retained-message scan of the current broker session.
#[derive(Debug, Default)]
struct Scan {
    /// Counts broker sessions, so a timeout from an earlier one is ignored.
    session: u64,
    /// Discovery waits until the scan is done, so a migrated entity is unloaded before its
    /// component arrives.
    done: bool,
    /// Per-entity configs found so far.
    legacy: BTreeSet<String>,
    /// Retained messages seen, so a settle check can tell whether more arrived.
    seen: u64,
    /// A [`BrokerEvent::LateMigrate`] is already on its way.
    late_pending: bool,
}

/// The components `previous` has that `current` does not have on the same platform.
fn gone(
    previous: &BTreeMap<String, String>,
    current: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    previous
        .iter()
        .filter(|(id, platform)| current.get(*id) != Some(*platform))
        .map(|(id, platform)| (id.clone(), platform.clone()))
        .collect()
}

struct Worker {
    state: crate::state::AppState,
    settings: MqttSettings,
    slug: String,
    bridge: DeviceBridge,
    client: Subscriptions<RumqttcClient>,
    /// For spawned `…/set` handlers to report back on.
    broker_tx: mpsc::UnboundedSender<BrokerEvent>,
    routes: Routes,
    published: Option<Published>,
    scan: Scan,
    /// The latest value per uid, so state can be republished after a broker reconnect.
    values: BTreeMap<u64, hc_proto::ValueUpdate>,
    appliance: ConnectionState,
    last_resync: Option<Instant>,
}

/// The broker client, remembering what this broker session is subscribed to.
///
/// Subscribing to a topic with a retained message delivers that message again, so a repeated
/// subscription to Home Assistant's status topic would trigger a resync, which would subscribe
/// again. Each topic is subscribed once per broker session.
struct Subscriptions<C> {
    client: C,
    subscribed: BTreeSet<String>,
}

impl<C> Subscriptions<C> {
    const fn new(client: C) -> Self {
        Self {
            client,
            subscribed: BTreeSet::new(),
        }
    }

    /// Forgets the subscriptions: a new broker session starts without any.
    fn reset(&mut self) {
        self.subscribed.clear();
    }
}

#[async_trait::async_trait]
impl<C: MqttClient + Send> MqttClient for Subscriptions<C> {
    async fn publish(
        &mut self,
        topic: &str,
        payload: &[u8],
        retain: bool,
    ) -> Result<(), MqttError> {
        self.client.publish(topic, payload, retain).await
    }

    async fn subscribe(&mut self, topic_filter: &str) -> Result<(), MqttError> {
        if self.subscribed.contains(topic_filter) {
            return Ok(());
        }
        self.client.subscribe(topic_filter).await?;
        self.subscribed.insert(topic_filter.to_owned());
        Ok(())
    }
}

/// Whether a resync happened within [`RESYNC_DEBOUNCE`] of `now`.
fn resynced_recently(last: Option<Instant>, now: Instant) -> bool {
    last.is_some_and(|at| now.saturating_duration_since(at) < RESYNC_DEBOUNCE)
}

impl Worker {
    async fn run(
        mut self,
        mut events: DeviceEventStream,
        mut broker: mpsc::UnboundedReceiver<BrokerEvent>,
    ) {
        loop {
            let result = tokio::select! {
                event = events.recv() => match event {
                    Some(event) => self.on_device(event).await,
                    None => return,
                },
                Some(event) = broker.recv() => self.on_broker(event).await,
            };
            if let Err(error) = result {
                tracing::warn!(device = %self.slug, %error, "MQTT publish failed");
            }
        }
    }

    async fn on_device(&mut self, event: DeviceEvent) -> Result<(), MqttError> {
        match event {
            DeviceEvent::Connected => {
                self.appliance = ConnectionState::Connected;
                self.announce(false).await?;
                self.publish_presence().await
            }
            DeviceEvent::Disconnected => {
                self.appliance = ConnectionState::Offline;
                self.publish_presence().await
            }
            DeviceEvent::Updates(updates) => {
                for update in &updates {
                    self.values.insert(update.uid, update.clone());
                }
                self.publish_values(updates.iter()).await
            }
            DeviceEvent::Redescribed => self.announce(false).await,
        }
    }

    async fn on_broker(&mut self, event: BrokerEvent) -> Result<(), MqttError> {
        match event {
            // A new broker session has no subscriptions, and the broker may have published our last
            // will. Re-establish everything.
            BrokerEvent::Connected => {
                self.client.reset();
                self.start_scan().await?;
                self.publish_presence().await?;
                self.republish_values().await
            }
            BrokerEvent::HomeAssistantOnline => {
                if !self.scan.done || resynced_recently(self.last_resync, Instant::now()) {
                    return Ok(());
                }
                self.resync().await
            }
            BrokerEvent::RetainedDiscovery(payload) => {
                // Seeds what to compare against, so an unchanged message is not sent again and a
                // component removed while Kessel was stopped is still retracted.
                if !self.scan.done {
                    self.scan.seen += 1;
                    self.published = Published::from_retained(payload);
                }
                Ok(())
            }
            BrokerEvent::Legacy(topic) => {
                self.scan.seen += 1;
                self.scan.legacy.insert(topic);
                if self.scan.done && !self.scan.late_pending {
                    self.scan.late_pending = true;
                    self.after(SCAN_SETTLE, BrokerEvent::LateMigrate);
                }
                Ok(())
            }
            BrokerEvent::LateMigrate => {
                self.scan.late_pending = false;
                self.migrate(true).await
            }
            BrokerEvent::Synced { session } | BrokerEvent::ScanCheck { session, .. }
                if session != self.scan.session || self.scan.done =>
            {
                Ok(())
            }
            BrokerEvent::Synced { session } => {
                let seen = self.scan.seen;
                self.after(SCAN_SETTLE, BrokerEvent::ScanCheck { session, seen });
                Ok(())
            }
            BrokerEvent::ScanCheck { session, seen } if seen != self.scan.seen => {
                let seen = self.scan.seen;
                self.after(SCAN_SETTLE, BrokerEvent::ScanCheck { session, seen });
                Ok(())
            }
            BrokerEvent::ScanCheck { .. } => self.finish_scan().await,
            BrokerEvent::ScanTimeout { session } => {
                if session != self.scan.session || self.scan.done {
                    return Ok(());
                }
                self.finish_scan().await
            }
            BrokerEvent::Lost => Ok(()),
            BrokerEvent::Set { topic, payload } => {
                self.spawn_set(topic, payload);
                Ok(())
            }
            BrokerEvent::SetDone { relative, outcome } => {
                self.on_set_done(&relative, outcome).await
            }
        }
    }

    /// Sends `event` to this worker after `delay`.
    fn after(&self, delay: Duration, event: BrokerEvent) {
        let worker = self.broker_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _closed = worker.send(event);
        });
    }

    /// Subscribes to the retained messages discovery depends on, then to a marker the worker
    /// publishes itself.
    ///
    /// The scan ends once the marker is back and no retained message has arrived for
    /// [`SCAN_SETTLE`], or after [`SCAN_TIMEOUT`] regardless.
    async fn start_scan(&mut self) -> Result<(), MqttError> {
        self.scan = Scan {
            session: self.scan.session.wrapping_add(1),
            ..Scan::default()
        };
        self.published = None;
        let session = self.scan.session;
        self.after(SCAN_TIMEOUT, BrokerEvent::ScanTimeout { session });
        let device = &self.routes.device;
        self.client
            .subscribe(&discovery::legacy_config_filter(device))
            .await?;
        self.client.subscribe(&self.routes.discovery).await?;
        self.client.subscribe(&self.routes.sync).await?;
        self.client
            .publish(&self.routes.sync, session.to_string().as_bytes(), false)
            .await
    }

    async fn finish_scan(&mut self) -> Result<(), MqttError> {
        self.scan.done = true;
        self.migrate(false).await
    }

    /// Announces, moving any per-entity configs found into the device message on the way.
    ///
    /// Home Assistant's order: `migrate_discovery` on each old topic (it unloads the entity and
    /// keeps its registry entry), the device message, then an empty retained payload on each.
    async fn migrate(&mut self, force: bool) -> Result<(), MqttError> {
        let legacy = std::mem::take(&mut self.scan.legacy);
        for topic in &legacy {
            self.client
                .publish(topic, discovery::MIGRATE_PAYLOAD, false)
                .await?;
        }
        self.last_resync = Some(Instant::now());
        self.announce(force || !legacy.is_empty()).await?;
        for topic in &legacy {
            self.client.publish(topic, &[], true).await?;
        }
        if !legacy.is_empty() {
            tracing::info!(
                device = %self.slug,
                migrated = legacy.len(),
                "moved per-entity discovery configs into the device message"
            );
        }
        Ok(())
    }

    /// Publishes everything Home Assistant needs from scratch.
    async fn resync(&mut self) -> Result<(), MqttError> {
        self.last_resync = Some(Instant::now());
        self.announce(true).await?;
        self.publish_presence().await?;
        self.republish_values().await
    }

    async fn republish_values(&mut self) -> Result<(), MqttError> {
        let values: Vec<hc_proto::ValueUpdate> = self.values.values().cloned().collect();
        self.publish_values(values.iter()).await
    }

    /// Publishes the device message when it changed (always when `force`), retracting removed
    /// components first, and subscribes the command topics.
    ///
    /// Waits for the scan: until it is done the broker's copy is not known, and a migrated entity
    /// must be unloaded before its component arrives.
    async fn announce(&mut self, force: bool) -> Result<(), MqttError> {
        if !self.scan.done {
            return Ok(());
        }
        let Some(entry) = self.state.devices().entry(&self.slug) else {
            return Ok(());
        };
        let mut info = entry.discovery_info();
        info.configuration_url
            .clone_from(&self.settings.configuration_url);
        let identity = entry.profile.identity();
        let names = CatalogueNames::for_device(&identity.brand, &identity.vib);
        let origin = hc_core::discovery::DiscoveryOrigin {
            name: hc_core::discovery::DiscoveryOrigin::KESSEL.name,
            sw_version: Some(crate::version()),
            support_url: Some(crate::SUPPORT_URL),
        };
        let announcement = hc_core::discovery::Announcement {
            mode: self.settings.discovery,
            info: &info,
            names: &names,
            origin: &origin,
        };
        let device = DeviceId::new(&self.slug);
        let message = hc_core::discovery::device_discovery(
            &self.settings.base,
            &device,
            &entry.mapping,
            &announcement,
        );
        let published = self.publish_device(&message, force).await?;
        self.published = Some(published);
        self.bridge
            .subscribe_command_topics(&mut self.client, &entry.mapping)
            .await
    }

    async fn publish_device(
        &mut self,
        message: &DeviceDiscovery,
        force: bool,
    ) -> Result<Published, MqttError> {
        let encode = |value: &serde_json::Value| {
            serde_json::to_vec(value).map_err(|error| MqttError::Client(error.to_string()))
        };
        let payload = encode(&message.payload)?;
        let platforms = message.platforms();
        let removed = self
            .published
            .as_ref()
            .map(|previous| gone(&previous.platforms, &platforms))
            .unwrap_or_default();
        if !removed.is_empty() {
            let retraction = encode(&message.retracting(&removed))?;
            self.client
                .publish(&message.config_topic, &retraction, true)
                .await?;
        }
        let changed = self
            .published
            .as_ref()
            .is_none_or(|previous| previous.payload != payload);
        if force || changed || !removed.is_empty() {
            self.client
                .publish(&message.config_topic, &payload, true)
                .await?;
            tracing::debug!(
                device = %self.slug,
                components = platforms.len(),
                retracted = removed.len(),
                "published discovery"
            );
        }
        Ok(Published { payload, platforms })
    }

    async fn publish_presence(&mut self) -> Result<(), MqttError> {
        let online = self.appliance == ConnectionState::Connected;
        self.bridge
            .publish_availability(&mut self.client, online)
            .await?;
        self.bridge
            .publish_connection_state(&mut self.client, self.appliance)
            .await
    }

    async fn publish_values<'u>(
        &mut self,
        updates: impl Iterator<Item = &'u hc_proto::ValueUpdate>,
    ) -> Result<(), MqttError> {
        let Some(entry) = self.state.devices().entry(&self.slug) else {
            return Ok(());
        };
        for update in updates {
            self.bridge
                .publish_value(&mut self.client, &entry.mapping, &entry.model, update)
                .await?;
        }
        Ok(())
    }

    /// Handles a `…/set` on its own task: the write waits for the appliance's answer.
    fn spawn_set(&self, topic: String, payload: String) {
        let state = self.state.clone();
        let base = self.settings.base.clone();
        let slug = self.slug.clone();
        let done = self.broker_tx.clone();
        tokio::spawn(async move {
            let outcome = super::route_inbound_set(&state, &base, &slug, &topic, &payload).await;
            let relative = super::relative_set_topic(&base, &slug, &topic)
                .unwrap_or_default()
                .to_owned();
            let _closed = done.send(BrokerEvent::SetDone { relative, outcome });
        });
    }

    /// Logs the outcome of a `…/set`, and on a refusal republishes the feature's real value so a
    /// control in Home Assistant snaps back to what the appliance is actually doing.
    async fn on_set_done(&mut self, relative: &str, outcome: SetRouting) -> Result<(), MqttError> {
        match &outcome {
            SetRouting::Accepted => {
                tracing::info!(device = %self.slug, topic = relative, "MQTT command accepted");
                return Ok(());
            }
            SetRouting::NotOurs => return Ok(()),
            SetRouting::Refused(reason) => {
                tracing::warn!(device = %self.slug, topic = relative, %reason, "MQTT command refused");
            }
            SetRouting::NoDevice | SetRouting::NoLiveSession => {
                tracing::warn!(
                    device = %self.slug,
                    topic = relative,
                    "MQTT command dropped: the appliance is not connected"
                );
            }
        }
        let Some(entry) = self.state.devices().entry(&self.slug) else {
            return Ok(());
        };
        let Some(uid) = entry
            .mapping
            .entries()
            .iter()
            .find(|mapped| mapped.command_topic.as_deref() == Some(relative))
            .map(|mapped| u64::from(mapped.uid))
        else {
            return Ok(());
        };
        let Some(update) = self.values.get(&uid).cloned() else {
            return Ok(());
        };
        self.bridge
            .publish_value(&mut self.client, &entry.mapping, &entry.model, &update)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records what reaches the broker.
    #[derive(Default)]
    struct Recorder {
        subscribed: Vec<String>,
        published: Vec<String>,
    }

    #[async_trait::async_trait]
    impl MqttClient for Recorder {
        async fn publish(&mut self, topic: &str, _: &[u8], _: bool) -> Result<(), MqttError> {
            self.published.push(topic.to_owned());
            Ok(())
        }

        async fn subscribe(&mut self, topic_filter: &str) -> Result<(), MqttError> {
            self.subscribed.push(topic_filter.to_owned());
            Ok(())
        }
    }

    fn platforms(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(id, platform)| ((*id).to_owned(), (*platform).to_owned()))
            .collect()
    }

    #[tokio::test]
    async fn a_topic_is_subscribed_once_per_broker_session() {
        let mut client = Subscriptions::new(Recorder::default());
        client.subscribe("homeassistant/status").await.unwrap();
        client.subscribe("homeassistant/status").await.unwrap();
        client
            .subscribe("kessel/dishwasher/power/set")
            .await
            .unwrap();
        assert_eq!(client.client.subscribed.len(), 2);

        client.reset();
        client.subscribe("homeassistant/status").await.unwrap();
        assert_eq!(
            client.client.subscribed.len(),
            3,
            "a new session subscribes again"
        );
    }

    #[tokio::test]
    async fn publishes_are_never_deduplicated() {
        let mut client = Subscriptions::new(Recorder::default());
        client
            .publish("kessel/dishwasher/power", b"ON", true)
            .await
            .unwrap();
        client
            .publish("kessel/dishwasher/power", b"ON", true)
            .await
            .unwrap();
        assert_eq!(client.client.published.len(), 2);
    }

    #[test]
    fn an_unchanged_device_retracts_nothing() {
        let published = platforms(&[("door", "sensor"), ("power", "switch")]);
        assert!(gone(&published, &published.clone()).is_empty());
    }

    #[test]
    fn a_removed_or_replatformed_component_is_retracted_on_its_old_platform() {
        let published = platforms(&[("door", "sensor"), ("power", "switch"), ("light", "switch")]);
        let current = platforms(&[
            ("door", "sensor"),
            ("power", "select"),
            ("salt", "binary_sensor"),
        ]);

        assert_eq!(
            gone(&published, &current),
            platforms(&[("light", "switch"), ("power", "switch")])
        );
    }

    fn routes() -> Routes {
        Routes::new("kessel", "dishwasher")
    }

    #[test]
    fn the_sync_marker_ends_its_own_session_scan() {
        assert!(matches!(
            routes().classify("kessel/dishwasher/_control/discovery_sync", b"3", false),
            Some(BrokerEvent::Synced { session: 3 })
        ));
    }

    #[test]
    fn only_the_retained_copy_of_the_device_message_is_read() {
        let topic = "homeassistant/device/kessel_dishwasher/config";
        assert!(matches!(
            routes().classify(topic, b"{}", true),
            Some(BrokerEvent::RetainedDiscovery(_))
        ));
        // The worker's own publish coming back.
        assert!(routes().classify(topic, b"{}", false).is_none());
    }

    #[test]
    fn a_per_entity_config_of_this_device_is_legacy_and_its_echoes_are_not() {
        let topic =
            "homeassistant/switch/dishwasher/Dishcare_Dishwasher_Setting_InteriorLight/config";
        let config =
            br#"{"unique_id":"dishwasher-x","device":{"identifiers":["kessel_dishwasher"]}}"#;
        assert!(matches!(
            routes().classify(topic, config, true),
            Some(BrokerEvent::Legacy(found)) if found == topic
        ));
        // The migration marker and the clear that follow it.
        assert!(
            routes()
                .classify(topic, hc_core::discovery::MIGRATE_PAYLOAD, false)
                .is_none()
        );
        assert!(routes().classify(topic, b"", true).is_none());
        // Another integration's device under the same node id is not touched.
        let foreign = br#"{"unique_id":"x","device":{"identifiers":["zigbee_dishwasher"]}}"#;
        assert!(routes().classify(topic, foreign, true).is_none());
    }

    #[test]
    fn a_set_topic_is_routed_as_a_command() {
        assert!(matches!(
            routes().classify(
                "kessel/dishwasher/Dishcare/Dishwasher/Setting/TimeLight/set",
                b"On",
                false
            ),
            Some(BrokerEvent::Set { .. })
        ));
        assert!(matches!(
            routes().classify("homeassistant/status", b"online", true),
            Some(BrokerEvent::HomeAssistantOnline)
        ));
    }

    #[test]
    fn a_birth_message_right_after_a_resync_is_ignored() {
        let now = Instant::now();
        assert!(!resynced_recently(None, now));
        assert!(resynced_recently(Some(now), now + Duration::from_secs(5)));
        assert!(!resynced_recently(Some(now), now + RESYNC_DEBOUNCE));
    }
}
