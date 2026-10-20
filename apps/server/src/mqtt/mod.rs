//! The MQTT bridge: Home Assistant discovery, retained state, and `…/set` commands.
//!
//! Each appliance gets its own broker connection, because an MQTT last will belongs to a connection
//! and every appliance has its own availability topic. Topics are built from the device slug only;
//! appliance identifiers and keys never reach a topic or a log line.

mod bridge;

pub use bridge::spawn_device_bridge;

use hc_core::discovery::DiscoveryMode;
use hc_crypto::Secret;

/// Where the broker is, and how to talk to it. The password is `Secret`-wrapped, so `Debug` is safe.
#[derive(Debug, Clone)]
pub struct MqttSettings {
    /// Broker host or IP. Its presence is what enables MQTT.
    pub host: String,
    /// Broker port.
    pub port: u16,
    /// Username, if the broker wants one.
    pub username: Option<String>,
    /// Password, if the broker wants one.
    pub password: Option<Secret<String>>,
    /// Topic root: every device topic is `<base>/<slug>/…`.
    pub base: String,
    /// How many entities Home Assistant is told about. Raw topics are published either way.
    pub discovery: DiscoveryMode,
    /// The device page's "Visit" link. Under the add-on it opens Kessel's panel.
    pub configuration_url: Option<String>,
}

impl MqttSettings {
    /// The variable whose presence enables MQTT.
    pub const HOST_ENV: &'static str = "KESSEL_MQTT_HOST";
    /// Broker port.
    pub const PORT_ENV: &'static str = "KESSEL_MQTT_PORT";
    /// Username.
    pub const USERNAME_ENV: &'static str = "KESSEL_MQTT_USERNAME";
    /// Password.
    pub const PASSWORD_ENV: &'static str = "KESSEL_MQTT_PASSWORD";
    /// Topic root.
    pub const BASE_ENV: &'static str = "KESSEL_MQTT_BASE";
    /// Discovery width (`full` | `curated`).
    pub const DISCOVERY_ENV: &'static str = "KESSEL_MQTT_DISCOVERY";
    /// Where Kessel's web UI is reachable, for the device page's link (`http://…:8099/`).
    pub const CONFIGURATION_URL_ENV: &'static str = "KESSEL_CONFIGURATION_URL";

    /// Plain MQTT. TLS to the broker is not offered; the broker is expected on the local network.
    pub const DEFAULT_PORT: u16 = 1883;

    /// Default topic root.
    pub const DEFAULT_BASE: &'static str = "kessel";

    /// Reads the settings from the environment. `None` when [`Self::HOST_ENV`] is unset.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// [`Self::from_env`] over an arbitrary lookup, so it can be tested without touching the
    /// process environment.
    ///
    /// Returns `None` when no host is set, and also when the port does not parse: connecting to a
    /// guessed port would look healthy while publishing nowhere. The caller reports which it was.
    #[must_use]
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let host = lookup(Self::HOST_ENV).filter(|host| !host.trim().is_empty())?;
        let port = match lookup(Self::PORT_ENV) {
            Some(raw) => raw.trim().parse::<u16>().ok()?,
            None => Self::DEFAULT_PORT,
        };
        let base = lookup(Self::BASE_ENV)
            .map(|base| base.trim().trim_matches('/').to_owned())
            .filter(|base| !base.is_empty())
            .unwrap_or_else(|| Self::DEFAULT_BASE.to_owned());
        Some(Self {
            host: host.trim().to_owned(),
            port,
            username: lookup(Self::USERNAME_ENV).filter(|user| !user.is_empty()),
            password: lookup(Self::PASSWORD_ENV)
                .filter(|password| !password.is_empty())
                .map(Secret::new),
            base,
            discovery: discovery_mode(lookup(Self::DISCOVERY_ENV).as_deref()),
            configuration_url: lookup(Self::CONFIGURATION_URL_ENV)
                .map(|url| url.trim().to_owned())
                .filter(|url| !url.is_empty()),
        })
    }
}

/// The discovery mode a configuration word selects. Anything but `curated` is `Full`: a typo
/// costs a wider entity list, never a missing entity.
pub(crate) fn discovery_mode(word: Option<&str>) -> DiscoveryMode {
    match word.map(str::trim) {
        Some(word) if word.eq_ignore_ascii_case("curated") => DiscoveryMode::Curated,
        _ => DiscoveryMode::Full,
    }
}

/// Feature names for Home Assistant, from the same catalogue the web UI uses, so a feature is
/// called the same thing everywhere.
///
/// Names only ever reach an entity's `name`, never its `unique_id`, entity id or topics, so a
/// corrected or translated name never breaks an automation.
#[derive(Debug, Clone, Copy)]
pub struct CatalogueNames {
    labels: crate::labels::DeviceLabels<'static>,
}

impl CatalogueNames {
    /// The catalogue for one appliance model.
    #[must_use]
    pub fn for_device(brand: &str, vib: &str) -> Self {
        Self {
            labels: crate::labels::embedded().for_device(brand, vib),
        }
    }

    /// The catalogue for an onboarded device, or the model-independent one if it is unknown.
    pub(crate) fn for_slug(state: &crate::state::AppState, slug: &str) -> Self {
        state.devices().entry(slug).map_or_else(
            || Self::for_device("", ""),
            |entry| {
                let identity = entry.profile.identity();
                Self::for_device(&identity.brand, &identity.vib)
            },
        )
    }
}

impl hc_core::discovery::FeatureNaming for CatalogueNames {
    fn feature_name(&self, key: &str) -> Option<String> {
        Some(self.labels.feature(key).text)
    }

    fn feature_description(&self, key: &str) -> Option<String> {
        self.labels.feature(key).description
    }

    fn member_name(&self, key: &str, member: &str) -> Option<String> {
        // Only contributed names: Home Assistant matches states against `options` exactly, so a
        // derived spelling would have to survive the same transform on the way back.
        let label = self.labels.member(key, member);
        (label.source != crate::labels::LabelSource::Derived).then_some(label.text)
    }

    fn program_name(&self, key: &str) -> Option<String> {
        Some(self.labels.program(key).text)
    }

    fn curation(&self, key: &str) -> hc_core::presentation::Curation {
        self.labels.curation(key)
    }

    fn icon(&self, key: &str) -> Option<String> {
        self.labels.icon(key)
    }

    fn device_class(&self, key: &str) -> Option<String> {
        self.labels.device_class(key)
    }
}

const SET_SUFFIX: &str = "/set";

/// Resolves `<base>/<slug>/<Feature/Path>/set` to the device-relative `<Feature/Path>/set`.
///
/// Matching is segment-exact: `dishwasher-2/…` is not `dishwasher`'s, `kessel2/…` is not under
/// `kessel`, and only a `…/set` topic resolves, so Kessel's own state publishes can never be read
/// back as commands.
#[must_use]
pub fn relative_set_topic<'t>(base: &str, slug: &str, topic: &'t str) -> Option<&'t str> {
    let prefix = format!("{base}/{slug}/");
    let relative = topic.strip_prefix(&prefix)?;
    (relative.ends_with(SET_SUFFIX) && relative.len() > SET_SUFFIX.len()).then_some(relative)
}

/// What routing an inbound `…/set` message did.
#[derive(Debug)]
#[non_exhaustive]
pub enum SetRouting {
    /// Not this device's topic, or not a `…/set` topic.
    NotOurs,
    /// No such device is onboarded.
    NoDevice,
    /// The device is not connected. Commands are never queued for later.
    NoLiveSession,
    /// The write was refused, locally or by the appliance. Carries the reason.
    Refused(String),
    /// The appliance accepted the write.
    Accepted,
}

/// Routes one inbound `…/set` message to the device's writer.
///
/// This is a path from a broker message to a physical appliance. The writer used is the one
/// installed for this device's own connection, so a mis-routed message lands nowhere rather than
/// on a sibling appliance.
pub async fn route_inbound_set(
    state: &crate::state::AppState,
    base: &str,
    slug: &str,
    topic: &str,
    payload: &str,
) -> SetRouting {
    let Some(relative) = relative_set_topic(base, slug, topic) else {
        return SetRouting::NotOurs;
    };
    let Some(entry) = state.devices().entry(slug) else {
        return SetRouting::NoDevice;
    };
    let Some(shared) = state.writers().writer(slug) else {
        return SetRouting::NoLiveSession;
    };
    let identity = entry.profile.identity();
    let names = CatalogueNames::for_device(&identity.brand, &identity.vib);
    let device = hc_core::bridge::SetContext::new(&entry.model, &entry.mapping).with_names(&names);
    let verdict =
        hc_core::bridge::handle_set(&mut *shared.lock().await, &device, relative, payload).await;
    match verdict {
        Ok(()) => SetRouting::Accepted,
        Err(error) => SetRouting::Refused(error.to_string()),
    }
}

/// What a device task tells its MQTT bridge.
#[derive(Debug)]
pub enum DeviceEvent {
    /// The appliance connection reached CONNECTED.
    Connected,
    /// The connection ended, or a connect attempt failed.
    Disconnected,
    /// Decoded values arrived.
    Updates(Vec<hc_proto::ValueUpdate>),
    /// The appliance revised its own description and the device's mapping has been rebuilt.
    Redescribed,
}

/// The sending half, held by the device task.
pub type DeviceEvents = tokio::sync::mpsc::UnboundedSender<DeviceEvent>;

/// The receiving half, held by the bridge.
pub type DeviceEventStream = tokio::sync::mpsc::UnboundedReceiver<DeviceEvent>;

/// A fresh event channel for one device.
///
/// Unbounded because the sender runs between two reads of the appliance socket and must never
/// block, and dropping values would leave Home Assistant silently wrong. The bridge drains it
/// without waiting on anything slow.
#[must_use]
pub fn device_events() -> (DeviceEvents, DeviceEventStream) {
    tokio::sync::mpsc::unbounded_channel()
}
