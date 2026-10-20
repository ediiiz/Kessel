//! The stored, editable configuration.
//!
//! An explicitly set environment variable wins over the file: the container's environment is how an
//! operator escapes a bad stored value (a wrong broker host saved through the UI cannot lock them
//! out), and the `KESSEL_*` variables remain a documented way to configure the daemon. The file is
//! what the UI writes.
//!
//! The broker password is stored because the daemon has to authenticate with it, but:
//!
//! - the file is written owner-only (`0600`);
//! - the API never returns it: [`ConfigView`] reports only whether one is set;
//! - a save that omits it keeps the stored one, since the UI cannot read it back. Clearing is explicit
//!   (see [`ConfigUpdate`]).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::mqtt::MqttSettings;

/// The environment variable naming the config file, following the `KESSEL_*` prefix.
pub const CONFIG_FILE_ENV: &str = "KESSEL_CONFIG_FILE";

/// Where the config lives when [`CONFIG_FILE_ENV`] is unset.
///
/// Beside the API token's default; in the add-on image both are redirected into the persisted `/data`
/// volume.
pub const DEFAULT_CONFIG_FILE: &str = "secrets/kessel-config.json";

/// The config file path: [`CONFIG_FILE_ENV`] if set, else [`DEFAULT_CONFIG_FILE`].
#[must_use]
pub fn config_file() -> PathBuf {
    std::env::var(CONFIG_FILE_ENV)
        .map_or_else(|_| PathBuf::from(DEFAULT_CONFIG_FILE), PathBuf::from)
}

/// Everything an operator can store.
///
/// Not the listen port, which has to agree with the add-on's `ingress_port`. Not the token path either:
/// a config file must not be able to move the credential that protects it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredConfig {
    /// The MQTT broker, when one is configured.
    pub mqtt: Option<StoredMqtt>,
    /// The stderr log filter (`trace` … `error`).
    pub log_level: Option<String>,
    /// Whether mutating requests must authenticate. `None` means no; see [`effective_require_auth`].
    #[serde(default)]
    pub require_auth: Option<bool>,
}

/// The stored broker settings. Field names mirror [`MqttSettings`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredMqtt {
    /// Host or IP. Its presence is what enables MQTT.
    pub host: String,
    /// Port. `None` means [`MqttSettings::DEFAULT_PORT`].
    pub port: Option<u16>,
    /// Username, when the broker wants one.
    pub username: Option<String>,
    /// Password, when the broker wants one. Never returned by the API.
    pub password: Option<String>,
    /// Topic root. `None` means [`MqttSettings::DEFAULT_BASE`].
    pub base: Option<String>,
    /// Discovery mode word (`full` | `curated`).
    pub discovery: Option<String>,
}

/// Loads the stored config. A missing file is the ordinary first-run state, not an error.
///
/// # Errors
///
/// An IO error other than "not found", or malformed JSON.
pub fn load(path: &Path) -> std::io::Result<StoredConfig> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} is not a valid Kessel config: {error}", path.display()),
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(StoredConfig::default()),
        Err(error) => Err(error),
    }
}

/// Saves the stored config atomically, owner-readable only.
///
/// `std::fs::write` truncates first, so a concurrent reader (or a kill mid-save) can see an empty or
/// partial file. Instead this writes a sibling staging file, sets its permissions, then
/// [`std::fs::rename`]s it over the destination, so a reader sees the whole old file or the whole new
/// one. The staging file is a sibling because `rename` is only atomic within one filesystem, and it is
/// unique per call (see `staging_path`) so two concurrent saves race only over complete files.
///
/// # Errors
///
/// An IO error, or a config that cannot be serialised.
pub fn save(path: &Path, config: &StoredConfig) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(config)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string()))?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let staged = staging_path(path);
    // Permissions are set before the file has its real name, so it is never briefly world-readable.
    std::fs::write(&staged, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Err(error) =
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600))
        {
            // Leave no half-prepared file behind on the failure path.
            drop(std::fs::remove_file(&staged));
            return Err(error);
        }
    }
    if let Err(error) = std::fs::rename(&staged, path) {
        drop(std::fs::remove_file(&staged));
        return Err(error);
    }
    Ok(())
}

/// The sibling file a save stages into before renaming it over `path`.
///
/// Beside the destination, so the [`std::fs::rename`] cannot cross a filesystem.
///
/// Unique per call, not just per process: with the pid alone, two threads saving at once would stage
/// to the same name and the second `rename` would fail with `ENOENT`. The pid separates processes and
/// a monotonic counter separates calls within one.
fn staging_path(path: &Path) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let ticket = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.{ticket}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Where the running daemon's broker came from.
///
/// Reported because the UI cannot infer it: `crate::hassio` may have found a broker the stored config
/// knows nothing about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum MqttSource {
    /// `KESSEL_MQTT_*` on the container; the top of the precedence order.
    Environment,
    /// The settings page's own file.
    Stored,
    /// Home Assistant's own broker, from `GET /services/mqtt` (`crate::hassio`).
    Supervisor,
    /// No broker: nothing is published. A supported state, and the default.
    #[default]
    Unconfigured,
}

/// The live broker resolution, as `main` performed it at startup.
///
/// Held in `crate::state::AppState` rather than recomputed per request, because resolving it involves
/// an HTTP call to the Supervisor. [`MqttBrokerView`] adds whether autodiscovery is on, which is
/// computed per request by [`view`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MqttStatus {
    /// Where it came from.
    pub source: MqttSource,
    /// The host in use, when there is one. Not a credential and not an appliance identifier.
    pub host: Option<String>,
    /// The port in use, when there is one.
    pub port: Option<u16>,
}

/// The broker the UI is told about: the live resolution, plus whether autodiscovery is on.
///
/// Nested because these facts describe the running daemon, while the rest of [`ConfigView`] describes
/// the stored file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MqttBrokerView {
    /// Where the running daemon's broker came from.
    pub source: MqttSource,
    /// The host it is connected to, whatever the source.
    pub host: Option<String>,
    /// The port that goes with it.
    pub port: Option<u16>,
    /// Whether Supervisor autodiscovery is switched on (the `mqtt_autodiscovery` add-on option).
    ///
    /// With it on, an empty host box means "Home Assistant's broker"; with it off, "no MQTT at all".
    pub autodiscovery: bool,
}

/// What `GET /api/config` returns. Carries no password, in any form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ConfigView {
    /// The broker host, if one is configured.
    pub mqtt_host: Option<String>,
    /// The broker port, if stored. Absent means the default applies.
    pub mqtt_port: Option<u16>,
    /// The broker username, if stored. Not a credential; it shows which user was tried.
    pub mqtt_username: Option<String>,
    /// Whether a password is stored. Never the value.
    pub mqtt_password_set: bool,
    /// The topic root, if stored.
    pub mqtt_base: Option<String>,
    /// The discovery mode word, if stored.
    pub mqtt_discovery: Option<String>,
    /// The stored log filter, if any.
    pub log_level: Option<String>,
    /// Whether mutating requests must present the API token (or a session cookie exchanged for it).
    ///
    /// The stored value; [`Self::overridden_by_env`] names `KESSEL_REQUIRE_AUTH` when the environment
    /// decides instead.
    pub require_auth: bool,
    /// The environment variables currently overriding stored settings, by name.
    pub overridden_by_env: Vec<String>,
    /// Whether the daemon needs restarting for the stored values to take effect.
    ///
    /// Always `true`: nothing is hot-reloaded. `POST /api/restart` applies it.
    pub restart_required: bool,
    /// The running daemon's broker, which the stored fields above cannot describe.
    pub mqtt: MqttBrokerView,
}

impl ConfigView {
    /// Overlays the live broker resolution `main` performed at startup.
    ///
    /// [`view`] is sync and cannot resolve a Supervisor broker, so it reports what is stored and this
    /// adds what is running. `autodiscovery` is left as [`view`] computed it.
    #[must_use]
    pub fn with_status(mut self, status: &MqttStatus) -> Self {
        self.mqtt.source = status.source;
        self.mqtt.host.clone_from(&status.host);
        self.mqtt.port = status.port;
        self
    }
}

/// What `PUT /api/config` accepts.
///
/// The password has three states: absent keeps, a value sets, an empty string clears. The UI cannot
/// read the password back, so "absent keeps" stops every save from wiping it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ConfigUpdate {
    /// The broker host. Empty or `null` removes the broker entirely.
    pub mqtt_host: Option<String>,
    /// The broker port; `null` means the default.
    pub mqtt_port: Option<u16>,
    /// The broker username; `null` means anonymous.
    pub mqtt_username: Option<String>,
    /// The broker password: absent keeps the stored one, a value sets it, `""` clears it.
    pub mqtt_password: Option<String>,
    /// The topic root; `null` means the default.
    pub mqtt_base: Option<String>,
    /// The discovery mode word; `null` means the default (`full`).
    pub mqtt_discovery: Option<String>,
    /// The log filter; `null` leaves it unset.
    pub log_level: Option<String>,
    /// Whether to require authentication for mutating requests: absent keeps the stored setting.
    ///
    /// Unlike most fields, absence keeps, so a client that omits it cannot switch the gate off by
    /// saving an unrelated change.
    #[serde(default)]
    pub require_auth: Option<bool>,
}

impl ConfigUpdate {
    /// Applies this update to `current`, carrying the stored password forward unless told otherwise.
    #[must_use]
    pub fn apply(self, current: &StoredConfig) -> StoredConfig {
        let stored_password = current.mqtt.as_ref().and_then(|mqtt| mqtt.password.clone());
        let host = self.mqtt_host.filter(|host| !host.trim().is_empty());
        StoredConfig {
            mqtt: host.map(|host| StoredMqtt {
                host: host.trim().to_owned(),
                port: self.mqtt_port,
                username: self.mqtt_username.filter(|user| !user.is_empty()),
                // The three states in one expression: `None` keeps, `Some("")` clears, `Some(v)` sets.
                password: match self.mqtt_password {
                    None => stored_password,
                    Some(password) if password.is_empty() => None,
                    Some(password) => Some(password),
                },
                base: self.mqtt_base.filter(|base| !base.trim().is_empty()),
                discovery: self.mqtt_discovery.filter(|word| !word.trim().is_empty()),
            }),
            log_level: self.log_level.filter(|level| !level.trim().is_empty()),
            // Absent keeps: see the field's docs.
            require_auth: self.require_auth.or(current.require_auth),
        }
    }
}

/// The view of `stored`, naming the environment variables it is currently losing to.
#[must_use]
pub fn view(stored: &StoredConfig, lookup: impl Fn(&str) -> Option<String>) -> ConfigView {
    let mut overridden: Vec<String> = [
        MqttSettings::HOST_ENV,
        MqttSettings::PORT_ENV,
        MqttSettings::USERNAME_ENV,
        MqttSettings::PASSWORD_ENV,
        MqttSettings::BASE_ENV,
        MqttSettings::DISCOVERY_ENV,
        crate::observability::LOG_ENV,
    ]
    .into_iter()
    .filter(|key| lookup(key).is_some_and(|value| !value.is_empty()))
    .map(str::to_owned)
    .collect();
    // Listed only if it parses: `require_auth_from_env` ignores an unrecognised word.
    if lookup(crate::auth::REQUIRE_AUTH_ENV)
        .and_then(|value| crate::auth::parse_require_auth(&value))
        .is_some()
    {
        overridden.push(crate::auth::REQUIRE_AUTH_ENV.to_owned());
    }
    let mqtt = stored.mqtt.as_ref();
    ConfigView {
        mqtt_host: mqtt.map(|mqtt| mqtt.host.clone()),
        mqtt_port: mqtt.and_then(|mqtt| mqtt.port),
        mqtt_username: mqtt.and_then(|mqtt| mqtt.username.clone()),
        // The one field that reports existence rather than content.
        mqtt_password_set: mqtt.is_some_and(|mqtt| mqtt.password.is_some()),
        mqtt_base: mqtt.and_then(|mqtt| mqtt.base.clone()),
        mqtt_discovery: mqtt.and_then(|mqtt| mqtt.discovery.clone()),
        log_level: stored.log_level.clone(),
        require_auth: stored.require_auth.unwrap_or(false),
        overridden_by_env: overridden,
        restart_required: true,
        // Source, host and port arrive via `ConfigView::with_status`; "no broker" here rather than a
        // guess from `stored` that could disagree with the running daemon.
        mqtt: MqttBrokerView {
            autodiscovery: crate::hassio::autodiscovery_enabled(lookup),
            ..MqttBrokerView::default()
        },
    }
}

/// Whether the daemon should authenticate mutating requests: the environment if it says anything, else
/// the file, else no.
///
/// "No" is the default because the add-on is reached through Home Assistant's ingress, which has
/// already authenticated the browser. Anyone running the daemon elsewhere, or behind a proxy that
/// does not authenticate, should turn it on. `KESSEL_REQUIRE_AUTH=false` also unlocks a daemon whose
/// token file was lost.
#[must_use]
pub fn effective_require_auth(stored: &StoredConfig) -> bool {
    crate::auth::require_auth_from_env().unwrap_or_else(|| stored.require_auth.unwrap_or(false))
}

/// The broker settings the daemon should use: the environment if it says anything, else the file.
///
/// `None` means neither configured one.
#[must_use]
pub fn effective_mqtt(stored: &StoredConfig) -> Option<MqttSettings> {
    // The environment wins as a whole, so a partial environment and a partial file never combine into
    // a configuration nobody wrote down.
    if let Some(from_env) = MqttSettings::from_env() {
        return Some(from_env);
    }
    let mqtt = stored.mqtt.as_ref()?;
    // Routed through the environment's parser so defaults, trimming and discovery-word matching have
    // one implementation.
    MqttSettings::from_lookup(|key| match key {
        MqttSettings::HOST_ENV => Some(mqtt.host.clone()),
        MqttSettings::PORT_ENV => mqtt.port.map(|port| port.to_string()),
        MqttSettings::USERNAME_ENV => mqtt.username.clone(),
        MqttSettings::PASSWORD_ENV => mqtt.password.clone(),
        MqttSettings::BASE_ENV => mqtt.base.clone(),
        MqttSettings::DISCOVERY_ENV => mqtt.discovery.clone(),
        _ => None,
    })
}
