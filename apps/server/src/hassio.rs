//! The Home Assistant Supervisor API: discovering Home Assistant's own MQTT broker, and restarting
//! the add-on.
//!
//! | endpoint | what it provides | what the add-on must declare |
//! |---|---|---|
//! | `GET /services/mqtt` | host, port, username, password of Home Assistant's broker | `services: [mqtt:want]` |
//! | `POST /addons/self/restart` | the Settings page's restart button | `hassio_api: true` |
//! | `GET /addons/self/info` | this add-on's slug, for the device page's link into its panel | `hassio_api: true` |
//!
//! There is no Supervisor in CI, so the HTTP calls are untested; the parsing on this side of the
//! socket is (`apps/server/tests/hassio.rs`). Every failure is a named refusal rather than a
//! fallback, so a wrong shape is reported instead of producing a daemon that looks configured and
//! publishes nothing.
//!
//! `http://supervisor` is plain HTTP on the Supervisor's internal network, so this speaks HTTP/1 over
//! a bare `TcpStream` with `hyper` (already linked by `axum`) rather than pulling in a TLS-capable
//! client. `send` refuses a non-`http` scheme rather than silently downgrading one.
//!
//! The broker password is [`Secret`]-wrapped at the parse boundary, because [`DiscoveredBroker`]
//! reaches the startup log.

use std::time::Duration;

use axum::body::Bytes;
use hc_crypto::Secret;
use http_body_util::BodyExt as _;
use serde::Deserialize;

use crate::config::StoredConfig;
use crate::mqtt::MqttSettings;

/// The variable the Supervisor sets on every add-on container. Its presence is the signal that the
/// daemon runs as an add-on.
pub const TOKEN_ENV: &str = "SUPERVISOR_TOKEN";

/// Overrides [`DEFAULT_BASE_URL`], for test harnesses and for running beside a Supervisor rather than
/// under one.
pub const BASE_URL_ENV: &str = "KESSEL_SUPERVISOR_URL";

/// Where the Supervisor answers, from inside an add-on container.
pub const DEFAULT_BASE_URL: &str = "http://supervisor";

/// The add-on option, as the entrypoint exports it. Absent means on; see [`autodiscovery_enabled`].
pub const AUTODISCOVER_ENV: &str = "KESSEL_MQTT_AUTODISCOVER";

/// The MQTT service descriptor.
const MQTT_SERVICE_PATH: &str = "/services/mqtt";

/// This add-on's own restart. `self` rather than the slug, so the slug is not duplicated here.
const SELF_RESTART_PATH: &str = "/addons/self/restart";

/// This add-on's own description.
const SELF_INFO_PATH: &str = "/addons/self/info";

/// How long any one Supervisor call may take.
///
/// Discovery sits on the startup path, so an unreachable Supervisor must cost ten seconds, not the
/// boot.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Everything that can go wrong talking to the Supervisor.
///
/// Every variant is a refusal with a reason: the caller logs it and carries on without a broker, which
/// is a supported state.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HassioError {
    /// No [`TOKEN_ENV`]: this is not an add-on container, so there is no Supervisor to ask.
    #[error("not running as a Home Assistant add-on ({TOKEN_ENV} is unset)")]
    NotAnAddon,
    /// The socket, the timeout, or a malformed base URL.
    #[error("the Supervisor could not be reached: {0}")]
    Unreachable(String),
    /// A non-2xx status, or an envelope whose `result` is not `ok`. Carries the Supervisor's own
    /// message (for example, that no MQTT broker is installed).
    #[error("the Supervisor refused: {message}")]
    Refused {
        /// The HTTP status, or `200` when the envelope itself said `error`.
        status: u16,
        /// The Supervisor's message, verbatim.
        message: String,
    },
    /// The answer arrived and could not be understood.
    #[error("the Supervisor's answer could not be understood: {0}")]
    Malformed(String),
    /// Home Assistant's broker wants TLS.
    ///
    /// Refused rather than dialled in the clear: this build's MQTT client has no TLS, and a plaintext
    /// connection to a TLS port would look like a broken broker rather than an unsupported setup.
    #[error("Home Assistant's MQTT broker requires TLS, which this build cannot speak")]
    BrokerWantsTls,
}

/// A broker the Supervisor handed over.
///
/// `Debug`-safe: the password is [`Secret`]-wrapped at the parse boundary, because this struct reaches
/// the startup log.
#[derive(Debug, Clone)]
pub struct DiscoveredBroker {
    /// The broker host on the Supervisor's network — `core-mosquitto` for the official add-on.
    pub host: String,
    /// The broker port.
    pub port: u16,
    /// The username the Supervisor minted for add-ons.
    pub username: Option<String>,
    /// Its password.
    pub password: Option<Secret<String>>,
}

impl DiscoveredBroker {
    /// Turns a discovered broker into settings, keeping the operator's topic root and discovery mode.
    ///
    /// The Supervisor supplies a broker, never a topic layout; changing the root would move every
    /// topic and duplicate the entities in Home Assistant.
    #[must_use]
    pub fn into_settings(self, stored: &StoredConfig) -> MqttSettings {
        let mqtt = stored.mqtt.as_ref();
        MqttSettings {
            host: self.host,
            port: self.port,
            username: self.username,
            password: self.password,
            base: mqtt
                .and_then(|mqtt| mqtt.base.clone())
                .map(|base| base.trim().trim_matches('/').to_owned())
                .filter(|base| !base.is_empty())
                .unwrap_or_else(|| MqttSettings::DEFAULT_BASE.to_owned()),
            discovery: crate::mqtt::discovery_mode(mqtt.and_then(|mqtt| mqtt.discovery.as_deref())),
            configuration_url: None,
        }
    }
}

/// The Supervisor's envelope. Every answer carries it.
#[derive(Deserialize)]
struct Envelope<T> {
    result: String,
    #[serde(default = "Option::default")]
    data: Option<T>,
    #[serde(default = "Option::default")]
    message: Option<String>,
}

/// The `data` of `GET /services/mqtt`.
///
/// A subset: `addon` and `protocol` are not needed. Unknown fields are ignored so a new Supervisor
/// field cannot break an installed add-on.
#[derive(Deserialize)]
struct MqttService {
    host: String,
    port: u16,
    #[serde(default = "bool::default")]
    ssl: bool,
    #[serde(default = "Option::default")]
    username: Option<String>,
    #[serde(default = "Option::default")]
    password: Option<String>,
}

/// Parses `GET /services/mqtt`'s body into a broker.
///
/// Split from the HTTP call so tests can drive it without a Supervisor.
///
/// # Errors
///
/// [`HassioError::Malformed`] if the bytes are not the envelope, [`HassioError::Refused`] if the
/// Supervisor said `error`, [`HassioError::BrokerWantsTls`] if the broker wants a transport this build
/// does not have.
pub fn parse_mqtt_service(body: &[u8]) -> Result<DiscoveredBroker, HassioError> {
    let envelope: Envelope<MqttService> =
        serde_json::from_slice(body).map_err(|error| HassioError::Malformed(error.to_string()))?;
    if envelope.result != "ok" {
        return Err(HassioError::Refused {
            status: 200,
            message: envelope
                .message
                .unwrap_or_else(|| "no MQTT service is available".to_owned()),
        });
    }
    let service = envelope.data.ok_or_else(|| {
        HassioError::Malformed("the answer said `ok` and carried no service".to_owned())
    })?;
    if service.ssl {
        return Err(HassioError::BrokerWantsTls);
    }
    Ok(DiscoveredBroker {
        host: service.host,
        port: service.port,
        username: service.username.filter(|user| !user.is_empty()),
        // Wrapped the moment it leaves the parser, so there is no unprotected binding to leak.
        password: service
            .password
            .filter(|password| !password.is_empty())
            .map(Secret::new),
    })
}

/// The `data` of `GET /addons/self/info`; only the slug is needed.
#[derive(Deserialize)]
struct SelfInfo {
    slug: String,
}

/// The slug out of `GET /addons/self/info`'s body: `local_kessel`, or `<repository>_kessel` for an
/// add-on installed from a repository.
///
/// # Errors
///
/// [`HassioError::Malformed`] for anything but an `ok` envelope carrying a non-empty slug.
pub fn parse_self_slug(body: &[u8]) -> Result<String, HassioError> {
    let envelope: Envelope<SelfInfo> =
        serde_json::from_slice(body).map_err(|error| HassioError::Malformed(error.to_string()))?;
    envelope
        .data
        .filter(|_| envelope.result == "ok")
        .map(|info| info.slug)
        .filter(|slug| !slug.trim().is_empty())
        .ok_or_else(|| HassioError::Malformed("the answer carried no add-on slug".to_owned()))
}

/// The link that opens this add-on's panel inside Home Assistant.
#[must_use]
pub fn ingress_url(slug: &str) -> String {
    format!("homeassistant://hassio/ingress/{slug}")
}

/// Asks the Supervisor which add-on this is, for [`ingress_url`].
///
/// # Errors
///
/// Any [`HassioError`]; the device page then has no link, and nothing else changes.
pub async fn fetch_ingress_url() -> Result<String, HassioError> {
    let token = token().ok_or(HassioError::NotAnAddon)?;
    let (status, body) = send(&base_url(), &token, http::Method::GET, SELF_INFO_PATH).await?;
    if !status.is_success() {
        return Err(HassioError::Refused {
            status: status.as_u16(),
            message: supervisor_message(&body)
                .unwrap_or_else(|| format!("HTTP {}", status.as_u16())),
        });
    }
    parse_self_slug(&body).map(|slug| ingress_url(&slug))
}

/// Whether the add-on option leaves autodiscovery on. Anything but a negative word is on, because a
/// typo that silently disabled discovery would look like a broken add-on.
#[must_use]
pub fn autodiscovery_enabled(lookup: impl Fn(&str) -> Option<String>) -> bool {
    !lookup(AUTODISCOVER_ENV).is_some_and(|word| {
        matches!(
            word.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        )
    })
}

/// Whether the Supervisor should be asked at all.
///
/// Autodiscovery fills a gap; it never overrules the operator. The precedence is:
///
/// 1. `KESSEL_MQTT_HOST` and friends;
/// 2. the stored broker the settings page saved;
/// 3. the Supervisor.
#[must_use]
pub fn should_autodiscover(stored: &StoredConfig, lookup: impl Fn(&str) -> Option<String>) -> bool {
    let typed = stored
        .mqtt
        .as_ref()
        .is_some_and(|mqtt| !mqtt.host.trim().is_empty());
    let from_env = lookup(MqttSettings::HOST_ENV).is_some_and(|host| !host.trim().is_empty());
    !typed && !from_env && autodiscovery_enabled(lookup)
}

/// Resolves the broker this process will use, and says where it came from.
///
/// `main` calls this once, and the device bridges, the startup log and the settings page all read the
/// same answer.
///
/// Never fails: every Supervisor failure is logged and becomes "no broker", a supported state.
pub async fn resolve_mqtt(
    stored: &StoredConfig,
) -> (Option<MqttSettings>, crate::config::MqttStatus) {
    use crate::config::{MqttSource, MqttStatus};

    let status = |source, settings: &MqttSettings| MqttStatus {
        source,
        host: Some(settings.host.clone()),
        port: Some(settings.port),
    };

    // 1 and 2: the environment as a whole, then the file.
    if let Some(settings) = crate::config::effective_mqtt(stored) {
        let source = if MqttSettings::from_env().is_some() {
            MqttSource::Environment
        } else {
            MqttSource::Stored
        };
        let live = status(source, &settings);
        return (Some(settings), live);
    }

    // 3: the Supervisor.
    if !should_autodiscover(stored, |key| std::env::var(key).ok()) {
        return (None, MqttStatus::default());
    }
    match fetch_mqtt_broker().await {
        Ok(broker) => {
            let settings = broker.into_settings(stored);
            let live = status(MqttSource::Supervisor, &settings);
            tracing::info!(
                host = %settings.host,
                port = settings.port,
                "Home Assistant's own MQTT broker was discovered through the Supervisor; nothing had \
                 to be configured. Set a host in Settings, or {}, to use a different broker",
                MqttSettings::HOST_ENV
            );
            (Some(settings), live)
        }
        // Logged, never fatal. Running outside the Supervisor and having no MQTT add-on are ordinary,
        // so they are `info` rather than `warn`.
        Err(error @ (HassioError::NotAnAddon | HassioError::Refused { .. })) => {
            tracing::info!(%error, "no broker was discovered automatically");
            (None, MqttStatus::default())
        }
        Err(error) => {
            tracing::warn!(
                %error,
                var = AUTODISCOVER_ENV,
                "the Supervisor was asked for Home Assistant's MQTT broker and the answer could not \
                 be used; configure a broker in Settings, or turn autodiscovery off"
            );
            (None, MqttStatus::default())
        }
    }
}

/// The Supervisor's base URL: [`BASE_URL_ENV`] if set, else [`DEFAULT_BASE_URL`].
#[must_use]
pub fn base_url() -> String {
    std::env::var(BASE_URL_ENV)
        .ok()
        .map(|url| url.trim().to_owned())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned())
}

/// The Supervisor token, or `None` when this is not an add-on container.
#[must_use]
pub fn token() -> Option<String> {
    std::env::var(TOKEN_ENV)
        .ok()
        .filter(|token| !token.trim().is_empty())
}

/// Asks the Supervisor for Home Assistant's broker.
///
/// # Errors
///
/// Any [`HassioError`]. Every one of them means "carry on without a broker", and the caller says so.
pub async fn fetch_mqtt_broker() -> Result<DiscoveredBroker, HassioError> {
    let token = token().ok_or(HassioError::NotAnAddon)?;
    let (status, body) = send(&base_url(), &token, http::Method::GET, MQTT_SERVICE_PATH).await?;
    if !status.is_success() {
        return Err(HassioError::Refused {
            status: status.as_u16(),
            // The Supervisor puts a reason in the envelope even on a 400, e.g. "No MQTT service
            // available".
            message: supervisor_message(&body)
                .unwrap_or_else(|| format!("HTTP {}", status.as_u16())),
        });
    }
    parse_mqtt_service(&body)
}

/// Restarts this add-on through the Supervisor.
///
/// This kills the caller's own container, so in practice it never returns `Ok`; the caller must have
/// answered its own client already.
///
/// # Errors
///
/// Any [`HassioError`]. [`HassioError::Refused`] with a 403 means the add-on does not declare
/// `hassio_api: true`.
pub async fn restart_self() -> Result<(), HassioError> {
    let token = token().ok_or(HassioError::NotAnAddon)?;
    let (status, body) = send(&base_url(), &token, http::Method::POST, SELF_RESTART_PATH).await?;
    if status.is_success() {
        return Ok(());
    }
    Err(HassioError::Refused {
        status: status.as_u16(),
        message: supervisor_message(&body).unwrap_or_else(|| format!("HTTP {}", status.as_u16())),
    })
}

/// The `message` out of an envelope, when there is one to be had.
fn supervisor_message(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Envelope<serde_json::Value>>(body)
        .ok()?
        .message
        .filter(|message| !message.is_empty())
}

/// One HTTP/1 request to the Supervisor, over a fresh connection.
///
/// No pooling: the daemon makes a handful of calls in its life (at startup, and a restart).
///
/// # Errors
///
/// [`HassioError::Unreachable`] for anything below the status line, including a base URL that is not
/// plain `http`.
async fn send(
    base_url: &str,
    token: &str,
    method: http::Method,
    path: &str,
) -> Result<(http::StatusCode, Bytes), HassioError> {
    let uri: hyper::Uri = format!("{}{path}", base_url.trim_end_matches('/'))
        .parse()
        .map_err(|error| HassioError::Unreachable(format!("{base_url} is not a URL: {error}")))?;
    // Refused rather than downgraded: there is no client TLS here, so `https://` would be spoken to
    // in the clear.
    if uri.scheme_str() != Some("http") {
        return Err(HassioError::Unreachable(format!(
            "{base_url} is not a plain http:// URL, and this build speaks no client TLS"
        )));
    }
    let host = uri
        .host()
        .ok_or_else(|| HassioError::Unreachable(format!("{base_url} names no host")))?;
    let authority = uri
        .authority()
        .map_or_else(|| host.to_owned(), ToString::to_string);
    let port = uri.port_u16().unwrap_or(80);

    let stream = tokio::time::timeout(
        REQUEST_TIMEOUT,
        tokio::net::TcpStream::connect((host, port)),
    )
    .await
    .map_err(|_elapsed| HassioError::Unreachable(format!("{authority} did not answer in time")))?
    .map_err(|error| HassioError::Unreachable(error.to_string()))?;

    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
            .await
            .map_err(|error| HassioError::Unreachable(error.to_string()))?;
    // The connection task drives the socket while the request is in flight and ends when it is dropped.
    // Its error is the same one `send_request` reports, so it is not logged twice.
    tokio::spawn(async move {
        let _ = connection.await;
    });

    let request = hyper::Request::builder()
        .method(method)
        .uri(uri.path())
        // HTTP/1.1 requires it, and `hyper`'s low-level client does not add it.
        .header(hyper::header::HOST, authority)
        // The credential, and the only place it is used. Never logged.
        .header(hyper::header::AUTHORIZATION, format!("Bearer {token}"))
        .body(http_body_util::Full::new(Bytes::new()))
        .map_err(|error| HassioError::Unreachable(error.to_string()))?;

    let response = tokio::time::timeout(REQUEST_TIMEOUT, sender.send_request(request))
        .await
        .map_err(|_elapsed| HassioError::Unreachable("the Supervisor timed out".to_owned()))?
        .map_err(|error| HassioError::Unreachable(error.to_string()))?;
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|error| HassioError::Unreachable(error.to_string()))?
        .to_bytes();
    Ok((status, body))
}
