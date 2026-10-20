//! Kessel daemon entry point.
//!
//! Wires the observability stack, builds the axum application, starts one long-lived task per
//! onboarded device (with its own MQTT bridge when a broker is configured), and serves the API
//! until Ctrl-C or `SIGTERM`.
//!
//! A write from the API or from an MQTT `…/set` topic reaches the appliance, whatever its type.
//!
//! `state::AppState::new` installs a non-dialling connection tester and a non-launching device-task
//! launcher; only this file replaces them with the live ones. That keeps an ordinary `cargo test`
//! off real appliances, so do not move either live implementation into `AppState::new`.

#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "test asserts may panic; library code may not"
    )
)]

use std::time::Duration;

use server::{
    api, auth, config, hassio, live_transport, observability, onboarding, serve, state, store,
};

/// The crates the daemon is assembled from, in dependency order; reported in the startup banner.
#[must_use]
pub const fn linked_crates() -> [&'static str; 6] {
    [
        hc_crypto::CRATE_NAME,
        hc_proto::CRATE_NAME,
        hc_transport::CRATE_NAME,
        hc_profile::CRATE_NAME,
        hc_downloader::CRATE_NAME,
        hc_core::CRATE_NAME,
    ]
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let bus = observability::install();

    // The stored configuration, then the broker it implies. Environment variables override the
    // file; a missing file is the ordinary first-run state. Resolved before the state is built
    // because the device-task launcher carries the MQTT settings to devices onboarded later too.
    //
    // The config path is read from the environment only here and carried in `AppState`, so tests
    // never write to a developer's own config file.
    let config_path = config::config_file();
    let stored = config::load(&config_path).unwrap_or_else(|error| {
        tracing::error!(
            %error,
            "the stored configuration could not be read; continuing with the environment only"
        );
        config::StoredConfig::default()
    });
    // If neither the environment nor the file names a broker, the Home Assistant Supervisor is
    // asked for its own. An unreachable or absent Supervisor is logged and becomes "no broker".
    let (mut mqtt, mqtt_status) = hassio::resolve_mqtt(&stored).await;
    // Under the add-on, the device page in Home Assistant links to Kessel's own panel.
    if let Some(settings) = mqtt.as_mut()
        && settings.configuration_url.is_none()
        && hassio::token().is_some()
    {
        match hassio::fetch_ingress_url().await {
            Ok(url) => settings.configuration_url = Some(url),
            Err(error) => tracing::debug!(%error, "no link to the add-on panel for Home Assistant"),
        }
    }

    // Authentication is off unless the operator asked for it: ingress already authenticates the
    // browser and no port is published.
    let require_auth = config::effective_require_auth(&stored);
    let token = resolve_token(require_auth)?;

    // The live connection tester, device-task launcher and profile store. `AppState::new` defaults
    // to none of them, which keeps the test suite offline and out of the profile directory.
    let profiles = store::ProfileStore::from_env();
    let state = state::AppState::new(std::sync::Arc::clone(&bus))
        .with_token(token)
        .with_auth_required(require_auth)
        .with_config_path(config_path)
        .with_connection_tester(std::sync::Arc::new(
            live_transport::LiveConnectionTester::new(),
        ))
        .with_device_launcher(std::sync::Arc::new(
            live_transport::LiveDeviceLauncher::new(mqtt.clone()),
        ))
        .with_profile_store(profiles)
        // The running broker, which can differ from the stored one (e.g. when autodiscovered).
        .with_mqtt_status(mqtt_status);

    // Restore onboarded devices from disk before `spawn_device_tasks` starts a task for each.
    let restored = onboarding::load_stored_devices(&state);

    // One long-lived task per device: it holds the connection open, installs the device's writer,
    // and reconciles every NOTIFY into the device's shadow. Devices onboarded later get their task
    // at onboarding time.
    let device_tasks = live_transport::spawn_device_tasks(&state, mqtt.as_ref());

    // Cloned: shutdown is signalled through this same state after the router has taken its copy.
    let app = api::app(state.clone());

    let port = serve::configured_port();
    let listener = serve::bind_api_listener(port).await?;
    let bound = listener.local_addr()?;

    tracing::info!(
        crates = linked_crates().len(),
        restored,
        trace_buffer = bus.capacity(),
        %bound,
        "kessel listening"
    );
    tracing::info!(device_tasks, "connecting to {device_tasks} appliance(s)");
    // The settings render without the password (`hc_crypto::Secret`).
    if let Some(settings) = &mqtt {
        tracing::info!(
            host = %settings.host,
            port = settings.port,
            base = %settings.base,
            discovery = ?settings.discovery,
            "MQTT is configured: one broker connection per device, publishing discovery, availability, \
             connection state and values, and subscribing to each writable feature's set topic"
        );
    } else {
        tracing::warn!(
            var = server::mqtt::MqttSettings::HOST_ENV,
            autodiscovery = hassio::AUTODISCOVER_ENV,
            "no MQTT broker is configured or discoverable, so nothing is published to Home \
             Assistant. Under the add-on this is normally automatic — install the Mosquitto broker \
             add-on and restart. Otherwise set a host in Settings or that variable; if one IS set, \
             the configuration was refused — check the port"
        );
    }
    let stopping = state.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            // At the signal, not after the drain: it ends the event streams the drain waits on, and
            // tells the device tasks to close their sessions. Appliances track clients in a bounded
            // table, so an abandoned session is held until the appliance notices.
            stopping.begin_shutdown();
        })
        .await?;
    close_device_sessions().await;
    Ok(())
}

/// Gives the device tasks a moment to send their WebSocket Close frames.
///
/// A short, bounded sleep rather than a join: the Supervisor follows `SIGTERM` with a kill, and
/// there are no join handles to await. A task that misses the deadline dies with the process.
async fn close_device_sessions() {
    const GRACE: Duration = Duration::from_millis(500);
    tokio::time::sleep(GRACE).await;
    tracing::info!("device sessions closed; exiting");
}

/// The API token.
///
/// The token file is created only when authentication is on, so no credential sits on disk that
/// nothing checks.
///
/// # Errors
///
/// Only when the gate is on and the token file can neither be read nor created.
fn resolve_token(require_auth: bool) -> anyhow::Result<auth::ApiToken> {
    Ok(if require_auth {
        // Read from the file, or generated and written owner-only on first run. The path is logged;
        // the value never is.
        let token_path = auth::token_file();
        let token = auth::ApiToken::load_or_create(&token_path)?;
        tracing::info!(
            path = %token_path.display(),
            header = auth::TOKEN_HEADER,
            "mutating requests must present the API token from this file"
        );
        token
    } else {
        tracing::info!(
            var = auth::REQUIRE_AUTH_ENV,
            "authentication is off: any client that can reach this API may command a write. That is \
             the default, because the add-on is reachable only through Home Assistant's ingress — turn \
             it on in Settings, or with this variable, if anything else can reach the port"
        );
        // Never checked and never written to disk.
        auth::ApiToken::generate()
    })
}

/// Resolves on Ctrl-C or `SIGTERM`.
///
/// `SIGTERM` is how the Home Assistant Supervisor stops and restarts the add-on. If a handler
/// cannot be installed it is logged and the other signal is still awaited, rather than shutting
/// down at startup.
///
/// MQTT connections are not disconnected cleanly: they drop with the process, so the broker
/// publishes each device's last will and Home Assistant shows the appliance offline.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "the Ctrl-C handler could not be installed");
            // Never resolves: this arm is out of the race, and `terminate` below still decides.
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::error!(
                    %error,
                    "the SIGTERM handler could not be installed; a service stop will terminate this \
                     process rather than shutting it down"
                );
                std::future::pending::<()>().await;
            }
        }
    };
    // No `SIGTERM` outside Unix; the arm keeps the `select!` well-typed.
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    let signal = tokio::select! {
        () = ctrl_c => "SIGINT",
        () = terminate => "SIGTERM",
    };
    tracing::info!(
        signal,
        "shutting down: no new connections are accepted, open requests are finished and the appliance \
         sessions are closed"
    );
}

#[cfg(test)]
mod tests {
    use super::linked_crates;

    #[test]
    fn every_protocol_crate_is_linked() {
        assert_eq!(
            linked_crates().len(),
            6,
            "all six hc-* crates must be reachable from the daemon"
        );
    }
}
