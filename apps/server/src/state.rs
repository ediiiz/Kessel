//! The shared application state: the trace bus the SSE inspector reads from, and the device
//! registry the dashboard renders.
//!
//! A device is keyed by its slug and described by its role class. The registry has no field for a
//! deviceID, serial, MAC, psk or iv; those stay inside `hc_profile`'s `Secret<T>`-wrapped types.

use std::collections::BTreeMap;
use std::sync::Arc;

use hc_core::device::{ConnectionState, DeviceRole};
use hc_core::mapping::{DeviceId, DeviceMapping, Excludes};
use hc_core::rendered_value;
use hc_core::shadow::Shadow;
use hc_core::trace::TraceBus;
use hc_profile::{EntityModel, Profile};
use parking_lot::Mutex;

use crate::auth::{ApiToken, SessionRegistry};
use crate::labels::{self, Label, LabelSource};
use crate::onboarding::{ConnectionTester, DeviceLauncher, NoDeviceTasks, NoLiveTransport};
use crate::store::ProfileStore;
use crate::types::{
    DeviceConnectionState, DeviceSummary, EntityDescriptor, HostSource, LabelOrigin,
    ProgramDescriptor,
};
use crate::write::WriterRegistry;

/// One device the daemon knows about.
#[derive(Debug, Clone)]
pub struct DeviceEntry {
    /// The appliance role class. Carried for display and the programme start path; gates no write.
    pub role: DeviceRole,
    /// Its current lifecycle state.
    pub state: ConnectionState,
    /// Why the last connection attempt failed, if the most recent one did.
    ///
    /// `ConnectionState::Offline` alone cannot tell a powered-off appliance from one that rejects
    /// the handshake. This reaches `GET /api/devices`, so it must already be host-redacted, which
    /// `crate::live_transport`'s `LiveTestError` and `transport_failure` guarantee.
    pub last_failure: Option<String>,
    /// The parsed capability set; every feature is covered.
    pub model: EntityModel,
    /// The feature→topic/entity mapping derived from the model.
    ///
    /// Rebuilt when [`Self::absorb_descriptions`] takes a revision from the appliance, so the
    /// dashboard and the MQTT bridge, which both read it from here, agree on what a feature is.
    pub mapping: DeviceMapping,
    /// What the appliance has said about its own features since its profile archive was taken.
    ///
    /// Empty until a `NOTIFY /ro/descriptionChange` arrives, and always empty for a device with no
    /// live session, in which case [`Self::mapping`] is exactly the archive's.
    pub live: hc_core::mapping::LiveDescriptions,
    /// Where the host this device is dialled at came from.
    pub host_source: HostSource,
    /// The operator's port override, or `None` for the transport's default.
    ///
    /// Kept beside the profile because the profile is the appliance's own export. Read by
    /// [`crate::onboarding::target_for`], so it applies to reconnects as well as the onboarding test.
    pub port: Option<u16>,
    /// This appliance's identity digest, [`Profile::fingerprint`].
    ///
    /// The upsert key: re-onboarding an appliance already present updates its record rather than
    /// allocating `cooktop-2`. It is a digest, never the `haId`, so it is safe to log.
    pub fingerprint: String,
    /// The parsed profile, kept so a manual-IP retry can re-dial the same appliance.
    ///
    /// It holds `Secret<T>`-wrapped credentials; `Profile`'s `Debug` redacts them.
    pub profile: Profile,
}

impl DeviceEntry {
    /// Builds an entry from a parsed profile, deriving the total mapping (`Excludes::none`).
    #[must_use]
    pub fn from_profile(slug: &str, role: DeviceRole, profile: &Profile) -> Self {
        let model = profile.model().clone();
        let mapping = DeviceMapping::build(&DeviceId::new(slug), &model, &Excludes::none());
        let host_source = if profile.manual_host().is_some() {
            HostSource::Manual
        } else {
            HostSource::Derived
        };
        Self {
            role,
            state: ConnectionState::Offline,
            last_failure: None,
            model,
            mapping,
            live: hc_core::mapping::LiveDescriptions::default(),
            host_source,
            port: None,
            fingerprint: profile.fingerprint(),
            profile: profile.clone(),
        }
    }

    /// Folds a batch's description revisions in, rebuilding [`Self::mapping`] if any landed.
    ///
    /// Returns whether anything changed, so the caller re-announces discovery only then rather than
    /// on every value `NOTIFY`.
    ///
    /// The mapping is rebuilt from the archive plus the whole overlay, never patched in place, so
    /// the result is a pure function of (archive, overlay).
    pub fn absorb_descriptions(&mut self, slug: &str, updates: &[hc_proto::ValueUpdate]) -> bool {
        // Not `any`: that would stop at the first revision and leave the rest of the batch
        // unabsorbed.
        let mut changed = false;
        for update in updates {
            changed |= self.live.absorb(update);
        }
        if changed {
            self.mapping = DeviceMapping::build_with(
                &DeviceId::new(slug),
                &self.model,
                &Excludes::none(),
                &self.live,
            );
        }
        changed
    }

    /// Builder form: the same entry with an operator port override.
    #[must_use]
    pub const fn with_port(mut self, port: Option<u16>) -> Self {
        self.port = port;
        self
    }

    /// The dashboard view of this device.
    #[must_use]
    pub fn summary(&self, slug: &str) -> DeviceSummary {
        DeviceSummary {
            slug: slug.to_owned(),
            role: self.role.label().to_owned(),
            // Derived from the appliance's own declaration, not its class: a device is writable iff
            // its `DeviceDescription` declares a writable feature, i.e. the mapping has a `…/set`
            // topic. A fridge that declares every setting `access="none"` gets no write control.
            ever_writable: self.mapping.command_topics().next().is_some(),
            connection_state: DeviceConnectionState::from(self.state),
            last_failure: self.last_failure.clone(),
            entity_count: count(self.model.entities().len()),
            // Straight off the manifest. Brand and vib are not secret; `DeviceIdentity` wraps
            // `ha_id`, `serial_number` and `mac` instead.
            brand: self.profile.identity().brand.clone(),
            vib: self.profile.identity().vib.clone(),
        }
    }

    /// The appliance's non-secret identity, for the Home Assistant `device` block.
    ///
    /// The same two non-secret fields [`Self::summary`] puts on the REST wire; `DeviceInfo` has no
    /// field for a serial or a MAC.
    ///
    /// `sw_version` is the device description's version, not a firmware version: the appliance does
    /// not report its firmware over this protocol.
    #[must_use]
    pub fn discovery_info(&self) -> hc_core::discovery::DeviceInfo {
        let identity = self.profile.identity();
        hc_core::discovery::DeviceInfo {
            brand: Some(identity.brand.clone()),
            model: Some(identity.vib.clone()),
            sw_version: self.model.meta().version.clone(),
            configuration_url: None,
        }
    }

    /// The appliance's programmes, named, in the mapping's order.
    ///
    /// Read-only by design; see [`ProgramDescriptor`].
    #[must_use]
    pub fn program_descriptors(&self) -> Vec<ProgramDescriptor> {
        let identity = self.profile.identity();
        let names = labels::embedded().for_device(&identity.brand, &identity.vib);
        self.mapping
            .programs()
            .iter()
            .map(|program| {
                // An unnamed programme is still listed and still called something. The fallback
                // spelling comes from `hc_core::presentation` so Home Assistant uses the same one.
                let label = program.key.as_deref().map_or_else(
                    || Label {
                        text: hc_core::presentation::unnamed_program_label(program.uid),
                        source: LabelSource::Derived,
                        description: None,
                    },
                    |key| names.program(key),
                );
                ProgramDescriptor {
                    uid: u32::from(program.uid),
                    feature_key: program.key.clone(),
                    label: label.text,
                    label_source: LabelOrigin::from(label.source),
                    description: label.description,
                    // The appliance's own word, passed through; `start` is gated on it, so it is not
                    // reduced to a boolean.
                    execution: program.execution.clone(),
                }
            })
            .collect()
    }

    #[must_use]
    /// One descriptor per mapped feature, in uid order, each carrying its current value if `shadow`
    /// has one.
    ///
    /// `shadow` is `None` when the device has no running task; see [`EntityDescriptor::value`].
    pub fn entity_descriptors(&self, shadow: Option<&Mutex<Shadow>>) -> Vec<EntityDescriptor> {
        let identity = self.profile.identity();
        let names = labels::embedded().for_device(&identity.brand, &identity.vib);
        self.mapping
            .entries()
            .iter()
            .map(|entry| {
                let entity = self.model.entity(entry.uid);
                // A keyless entity has nothing to look a name up by, so it falls back to its uid
                // path (`hc_core::mapping::relative_key`) rather than a blank.
                let label = entry.key.as_deref().map_or_else(
                    || Label {
                        text: hc_core::presentation::unnamed_feature_label(entry.uid),
                        source: LabelSource::Derived,
                        description: None,
                    },
                    |key| names.feature(key),
                );
                // Keyed by the RAW member, and only for members the appliance actually declared — so
                // a stale contributed entry names nothing and cannot desynchronise from `options`.
                let option_labels = entry
                    .key
                    .as_deref()
                    .map(|key| names.member_labels(key, entry.options.iter().map(String::as_str)))
                    .unwrap_or_default();
                EntityDescriptor {
                    uid: u32::from(entry.uid),
                    feature_key: entry.key.clone(),
                    feature_path: entry
                        .state_topic
                        .as_deref()
                        .or(entry.command_topic.as_deref())
                        .map(strip_suffix_segment),
                    access: entity
                        .and_then(|entity| entity.access.as_ref())
                        .map(access_label),
                    writable: entry.command_topic.is_some(),
                    // The same renderer `DeviceBridge::publish_value` uses for MQTT, so an enumeration
                    // is the declared label here and on the topic, matching `options`.
                    value: shadow.and_then(|shadow| {
                        shadow
                            .lock()
                            .get(u64::from(entry.uid))
                            .map(|held| rendered_value(&self.model, entry.uid, held.value()))
                    }),
                    options: entry.options.clone(),
                    label: label.text,
                    label_source: LabelOrigin::from(label.source),
                    description: label.description,
                    option_labels,
                }
            })
            .collect()
    }
}

/// The devices the daemon serves, keyed by slug.
#[derive(Debug, Default)]
pub struct DeviceRegistry {
    devices: Mutex<BTreeMap<String, DeviceEntry>>,
}

impl DeviceRegistry {
    /// An empty registry — the state before onboarding.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces a device.
    pub fn insert(&self, slug: impl Into<String>, entry: DeviceEntry) {
        self.devices.lock().insert(slug.into(), entry);
    }

    /// How many devices are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.devices.lock().len()
    }

    /// Whether no device has been onboarded yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.devices.lock().is_empty()
    }

    /// Every device's dashboard summary, in slug order.
    #[must_use]
    pub fn summaries(&self) -> Vec<DeviceSummary> {
        self.devices
            .lock()
            .iter()
            .map(|(slug, entry)| entry.summary(slug))
            .collect()
    }

    /// A clone of one device's entry, if it is registered.
    #[must_use]
    pub fn entry(&self, slug: &str) -> Option<DeviceEntry> {
        self.devices.lock().get(slug).cloned()
    }

    /// Runs `update` against one device's entry, if it is registered.
    pub fn update<T>(&self, slug: &str, update: impl FnOnce(&mut DeviceEntry) -> T) -> Option<T> {
        self.devices.lock().get_mut(slug).map(update)
    }

    /// Whether a slug is already in use.
    #[must_use]
    pub fn contains(&self, slug: &str) -> bool {
        self.devices.lock().contains_key(slug)
    }

    /// The slug of the device with this fingerprint, if one is registered.
    ///
    /// Lets a re-uploaded profile update the existing device. Matched on the digest, so it holds for
    /// a re-exported archive whose bytes differ.
    #[must_use]
    pub fn slug_for_fingerprint(&self, fingerprint: &str) -> Option<String> {
        self.devices
            .lock()
            .iter()
            .find(|(_, entry)| entry.fingerprint == fingerprint)
            .map(|(slug, _)| slug.clone())
    }

    /// Removes a device, returning whether one was registered under that slug.
    pub fn remove(&self, slug: &str) -> bool {
        self.devices.lock().remove(slug).is_some()
    }

    /// Records a lifecycle transition. A slug that is not registered is ignored.
    ///
    /// Reaching `Connected` clears [`DeviceEntry::last_failure`]: the stored reason describes the
    /// most recent attempt, and a device that has since connected has no current failure.
    pub fn set_state(&self, slug: &str, state: ConnectionState) {
        if let Some(entry) = self.devices.lock().get_mut(slug) {
            entry.state = state;
            if matches!(state, ConnectionState::Connected) {
                entry.last_failure = None;
            }
        }
    }

    /// Records that a connection attempt failed, and why. A slug that is not registered is ignored.
    ///
    /// `reason` must already be host-redacted — see [`DeviceEntry::last_failure`].
    ///
    /// This is the only writer of that field and [`Self::set_state`] reaching `Connected` the only
    /// clearer; `set_state(slug, Offline)` would record the state and lose the explanation.
    pub fn set_offline(&self, slug: &str, reason: &str) {
        if let Some(entry) = self.devices.lock().get_mut(slug) {
            entry.state = ConnectionState::Offline;
            entry.last_failure = Some(reason.to_owned());
        }
    }
}

/// Each connected device's shadow, keyed by slug: the read side of the update pipeline, shared by
/// the MQTT publisher and the REST surface.
///
/// The lifetime differs from [`WriterRegistry`]'s on purpose:
///
/// - a writer entry lasts one connection, because a write to a dead connection must fail closed;
/// - a shadow entry lasts one device task, reconnects included, because the last reported value is
///   still the last reported value after a brief outage. Reachability is reported separately by
///   `hc_core::device::ConnectionState`.
///
/// No entry means the device has no task at all, i.e. a build with no device launcher.
///
/// The values are appliance data, so the `Debug` below renders only slugs and a count.
#[derive(Default)]
pub struct ShadowRegistry {
    shadows: Mutex<BTreeMap<String, Arc<Mutex<Shadow>>>>,
}

impl core::fmt::Debug for ShadowRegistry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Slugs and a count. Never a shadow — rendering one would take its lock inside a `Debug`.
        let shadows = self.shadows.lock();
        f.debug_struct("ShadowRegistry")
            .field("devices", &shadows.keys().collect::<Vec<_>>())
            .field("count", &shadows.len())
            .finish()
    }
}

impl ShadowRegistry {
    /// An empty registry — the state in which no device has a live session.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `shadow` as `slug`'s, replacing any previous entry.
    ///
    /// Replacement rather than rejection, so a restarted device task cannot leave the registry
    /// pointing at a shadow nothing writes to.
    pub fn install(&self, slug: impl Into<String>, shadow: Arc<Mutex<Shadow>>) {
        self.shadows.lock().insert(slug.into(), shadow);
    }

    /// Removes `slug`'s shadow — called when its connection ends.
    pub fn remove(&self, slug: &str) {
        self.shadows.lock().remove(slug);
    }

    /// `slug`'s shadow, if it has a live session.
    #[must_use]
    pub fn shadow(&self, slug: &str) -> Option<Arc<Mutex<Shadow>>> {
        self.shadows.lock().get(slug).map(Arc::clone)
    }

    /// How many devices have a live session.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shadows.lock().len()
    }

    /// Whether no device has a live session.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shadows.lock().is_empty()
    }
}

/// Everything a handler needs, cloned per request (every field is an `Arc`).
#[derive(Debug, Clone)]
pub struct AppState {
    bus: Arc<TraceBus>,
    devices: Arc<DeviceRegistry>,
    auth: Arc<ApiToken>,
    writers: Arc<WriterRegistry>,
    shadows: Arc<ShadowRegistry>,
    tester: Arc<dyn ConnectionTester>,
    launcher: Arc<dyn DeviceLauncher>,
    sessions: Arc<SessionRegistry>,
    store: Option<Arc<ProfileStore>>,
    auth_required: bool,
    mqtt: crate::config::MqttStatus,
    config_path: Arc<std::path::PathBuf>,
    /// Broadcasts "stop" to every device task (`hc_core::supervisor::DeviceConfig::shutdown`).
    ///
    /// The device tasks are detached (`crate::live_transport::spawn_device_task` drops the
    /// `Supervisor`), so a shared signal is the only way to reach them. Every task takes a receiver
    /// from this one sender.
    shutdown: Arc<tokio::sync::watch::Sender<bool>>,
}

impl AppState {
    /// Builds the state around a trace bus, an empty registry and a **freshly generated** token.
    ///
    /// A state built without an explicit token is closed rather than open. Production replaces it
    /// with the on-disk token via [`with_token`](Self::with_token).
    #[must_use]
    pub fn new(bus: Arc<TraceBus>) -> Self {
        Self {
            bus,
            devices: Arc::new(DeviceRegistry::new()),
            auth: Arc::new(ApiToken::generate()),
            writers: Arc::new(WriterRegistry::new()),
            shadows: Arc::new(ShadowRegistry::new()),
            tester: Arc::new(NoLiveTransport),
            launcher: Arc::new(NoDeviceTasks),
            sessions: Arc::new(SessionRegistry::new()),
            store: None,
            // "No broker". `main` replaces it with the real resolution.
            mqtt: crate::config::MqttStatus::default(),
            shutdown: Arc::new(tokio::sync::watch::channel(false).0),
            // Closed by default, like the token above. The deployment default comes from the stored
            // config (`crate::config::effective_require_auth`), so running ungated is decided in
            // one place.
            auth_required: true,
            // `main` sets it explicitly from the same function; a test points it at a temp file.
            config_path: Arc::new(crate::config::config_file()),
        }
    }

    /// The file the config routes read and write.
    ///
    /// State rather than an ambient read of `KESSEL_CONFIG_FILE`, so tests can point it at a temp
    /// file instead of a developer's real configuration (`std::env::set_var` is `unsafe` and these
    /// crates forbid unsafe code). The environment is consulted only in [`Self::new`]'s default.
    #[must_use]
    pub fn with_config_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.config_path = Arc::new(path.into());
        self
    }

    /// The file the config routes read and write.
    #[must_use]
    pub fn config_path(&self) -> &std::path::Path {
        &self.config_path
    }

    /// Whether mutating requests must authenticate at all.
    ///
    /// `main` sets this from the stored configuration; `false` is the shipped default. When it is
    /// `false` the token and sessions still work, but nothing checks them (see
    /// `api::gate_mutations`).
    #[must_use]
    pub const fn with_auth_required(mut self, required: bool) -> Self {
        self.auth_required = required;
        self
    }

    /// Whether the mutating-request gate is switched on.
    #[must_use]
    pub const fn auth_required(&self) -> bool {
        self.auth_required
    }

    /// Records which broker this process resolved, and where it came from.
    ///
    /// Set once by `main` from `crate::hassio::resolve_mqtt`, so `GET /api/config` reports the broker
    /// actually in use, which on a fresh add-on install differs from the settings file.
    #[must_use]
    pub fn with_mqtt_status(mut self, status: crate::config::MqttStatus) -> Self {
        self.mqtt = status;
        self
    }

    /// The live broker resolution. `MqttSource::Unconfigured` in any state nobody told.
    #[must_use]
    pub const fn mqtt_status(&self) -> &crate::config::MqttStatus {
        &self.mqtt
    }

    /// Installs the profile store: `main` points it at `/data`, tests at a `TempDir`.
    ///
    /// `None` is the default, like [`NoLiveTransport`] and [`NoDeviceTasks`], so a test that onboards
    /// a fixture writes nothing into the repository.
    #[must_use]
    pub fn with_profile_store(mut self, store: ProfileStore) -> Self {
        self.store = Some(Arc::new(store));
        self
    }

    /// The profile store, or `None` in a build that persists nothing.
    ///
    /// A caller that finds `None` must still succeed: the daemon simply forgets on restart.
    #[must_use]
    pub const fn profile_store(&self) -> Option<&Arc<ProfileStore>> {
        self.store.as_ref()
    }

    /// Replaces the onboarding connection tester: `main` injects the live one, tests a fake.
    #[must_use]
    pub fn with_connection_tester(mut self, tester: Arc<dyn ConnectionTester>) -> Self {
        self.tester = tester;
        self
    }

    /// The onboarding connection tester.
    #[must_use]
    pub fn connection_tester(&self) -> &Arc<dyn ConnectionTester> {
        &self.tester
    }

    /// Replaces the device-task launcher: `main` injects the live one, tests a fake.
    ///
    /// The default is [`NoDeviceTasks`], which starts nothing. See [`DeviceLauncher`] for why the live
    /// spawner must not be the default.
    #[must_use]
    pub fn with_device_launcher(mut self, launcher: Arc<dyn DeviceLauncher>) -> Self {
        self.launcher = launcher;
        self
    }

    /// The device-task launcher onboarding starts a new device's task through.
    #[must_use]
    pub fn device_launcher(&self) -> &Arc<dyn DeviceLauncher> {
        &self.launcher
    }

    /// The per-device writer registry.
    ///
    /// There is no `with_writers`: an empty registry is already fail-closed, a device task installs
    /// its own entry on CONNECTED, and a test uses
    /// [`WriterRegistry::install`](crate::write::WriterRegistry::install) like production does.
    ///
    /// Every writer is a `hc_core::write::GuardedWriter`, so `GuardedWriter::write` is the only way
    /// to reach the socket from here.
    #[must_use]
    pub const fn writers(&self) -> &Arc<WriterRegistry> {
        &self.writers
    }

    /// Each connected device's shadow.
    ///
    /// No `with_shadows`, for the same reason as `writers`; a test uses [`ShadowRegistry::install`].
    #[must_use]
    pub const fn shadows(&self) -> &Arc<ShadowRegistry> {
        &self.shadows
    }

    /// Replaces the API token — production reads it from disk, tests inject a known value.
    #[must_use]
    pub fn with_token(mut self, token: ApiToken) -> Self {
        self.auth = Arc::new(token);
        self
    }

    /// The API token every mutating request must present.
    #[must_use]
    pub fn auth(&self) -> &ApiToken {
        &self.auth
    }

    /// The open browser sessions.
    ///
    /// No `with_sessions`; a test opens one through [`SessionRegistry::open`], as
    /// `POST /api/session` does.
    #[must_use]
    pub const fn sessions(&self) -> &Arc<SessionRegistry> {
        &self.sessions
    }

    /// The trace bus the SSE frame inspector subscribes to.
    #[must_use]
    pub const fn bus(&self) -> &Arc<TraceBus> {
        &self.bus
    }

    /// The device registry.
    #[must_use]
    pub const fn devices(&self) -> &Arc<DeviceRegistry> {
        &self.devices
    }

    /// A receiver every device task selects on, so shutdown interrupts a live session.
    #[must_use]
    pub fn shutdown_signal(&self) -> tokio::sync::watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    /// Tells every device task to close its session and stop.
    ///
    /// Idempotent, and safe with no tasks running.
    pub fn begin_shutdown(&self) {
        let _no_receivers = self.shutdown.send(true);
    }
}

/// Narrows a count for the wire, saturating rather than wrapping.
fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// `BSH/Common/Setting/PowerState/state` → `BSH/Common/Setting/PowerState`.
fn strip_suffix_segment(topic: &str) -> String {
    topic
        .rsplit_once('/')
        .map_or_else(|| topic.to_owned(), |(head, _)| head.to_owned())
}

/// The declared `access` word, verbatim — the same spelling the XML used.
fn access_label(access: &hc_profile::Access) -> String {
    match access {
        hc_profile::Access::None => "none".to_owned(),
        hc_profile::Access::Read => "read".to_owned(),
        hc_profile::Access::ReadStatic => "readStatic".to_owned(),
        hc_profile::Access::WriteOnly => "writeOnly".to_owned(),
        hc_profile::Access::ReadWrite => "readWrite".to_owned(),
        other => format!("{other:?}"),
    }
}
