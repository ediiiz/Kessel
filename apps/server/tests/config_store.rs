//! The stored, editable configuration.
//!
//! The properties tested, in order of how badly they would hurt:
//!
//! 1. the MQTT password never leaves the process: it is a credential and the API is reachable from
//!    the container network;
//! 2. a save that does not carry the password does not wipe it: a UI cannot read the password back,
//!    so every save would otherwise clear it;
//! 3. an explicitly-set environment variable still wins, as the escape hatch from a bad stored
//!    value.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

mod common;

use server::config::{StoredConfig, StoredMqtt};

/// A config with a broker and a password, as the UI would first save it.
fn configured() -> StoredConfig {
    StoredConfig {
        mqtt: Some(StoredMqtt {
            host: "192.0.2.2".to_owned(),
            port: Some(1884),
            username: Some("kessel".to_owned()),
            password: Some("s3cret-do-not-print".to_owned()),
            base: Some("kessel".to_owned()),
            discovery: Some("curated".to_owned()),
        }),
        log_level: Some("debug".to_owned()),
        require_auth: None,
    }
}

#[test]
fn a_stored_config_is_read_back() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("config.json");

    server::config::save(&path, &configured()).expect("the config saves");
    let loaded = server::config::load(&path).expect("the config loads");

    let mqtt = loaded.mqtt.expect("the broker survived the round trip");
    assert_eq!(mqtt.host, "192.0.2.2");
    assert_eq!(mqtt.port, Some(1884));
    assert_eq!(mqtt.username.as_deref(), Some("kessel"));
    assert_eq!(mqtt.base.as_deref(), Some("kessel"));
    assert_eq!(mqtt.discovery.as_deref(), Some("curated"));
    assert_eq!(loaded.log_level.as_deref(), Some("debug"));
    assert_eq!(
        mqtt.password.as_deref(),
        Some("s3cret-do-not-print"),
        "the password must survive on disk — it is what the daemon authenticates with"
    );
}

/// **A save is atomic: a concurrent reader never sees a half-written config.**
///
/// `std::fs::write` truncates before it writes, so a reader racing a save can see an empty file,
/// fail to parse it, and the API then returns 500. On a running daemon the same window can lose the
/// stored broker credentials. `save` writes a temporary file and renames it over the old one, so a
/// reader sees either the old file or the new one; the loop count is for confidence, not for
/// hitting a race.
#[test]
fn a_save_is_atomic_so_a_concurrent_read_never_sees_a_torn_file() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("config.json");
    server::config::save(&path, &configured()).expect("the initial config saves");

    // Two writers, so two threads in one process must not stage to the same temp path.
    let writers: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || {
                for _ in 0..2_000 {
                    server::config::save(&path, &configured()).expect("a save must not fail");
                }
            })
        })
        .collect();

    for reads in 0..2_000_usize {
        let loaded = server::config::load(&path).unwrap_or_else(|error| {
            panic!(
                "a read that raced a save saw a torn file after {reads} clean reads: {error}. \
                 On a running daemon this is the operator's stored broker credentials reported as \
                 unreadable."
            )
        });
        assert!(
            loaded.mqtt.is_some(),
            "a read that raced a save parsed, but came back empty — the file was caught after \
             truncation and before the write"
        );
    }
    for writer in writers {
        writer.join().expect("a concurrent save must not fail");
    }
}

/// **The config route reads and writes the path its state names, not an ambient one.**
///
/// Reading `KESSEL_CONFIG_FILE` inside the route would make `cargo test` write to the developer's
/// real configuration, and make every parallel test binary share one mutable file. Integration tests
/// cannot set the env var (`set_var` is `unsafe` in edition 2024), so the path lives on `AppState`;
/// `main` still resolves it from `config_file()`.
#[tokio::test]
async fn the_config_route_uses_the_path_its_state_names() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use server::auth::ApiToken;
    use server::state::AppState;

    const TOKEN: &str = "test-token-for-the-config-path-seam";
    /// An update that changes nothing: `ConfigUpdate::apply` carries every stored field forward.
    const NOTHING: &str = r#"{"mqtt_host":null,"mqtt_port":null,"mqtt_username":null,"mqtt_password":null,"mqtt_base":null,"mqtt_discovery":null,"log_level":null}"#;
    /// An update that stores a broker, sent only once the route is proved confined.
    const CONFIGURED: &str = r#"{"mqtt_host":"10.0.0.9","mqtt_port":1885,"mqtt_username":null,"mqtt_password":null,"mqtt_base":null,"mqtt_discovery":null,"log_level":"trace"}"#;

    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("kessel-config.json");
    let state = AppState::new(common::bus())
        .with_token(ApiToken::from_string(TOKEN))
        .with_config_path(path.clone());

    // **The assertion order is load-bearing, not stylistic.** The first PUT is all-nulls, which
    // `ConfigUpdate::apply` carries every stored field through unchanged — so while this test is red
    // it writes the operator's real config back byte for byte rather than damaging it. Only after
    // the *first* assertion proves the route is confined to the state's own path does the second PUT
    // store a value, and a red run panics before reaching it.
    let put = |body: &'static str| {
        Request::builder()
            .method("PUT")
            .uri("/api/config")
            .header(server::auth::TOKEN_HEADER, TOKEN)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .expect("a well-formed request")
    };

    let saved = common::send(server::api::app(state.clone()), put(NOTHING)).await;
    assert_eq!(
        saved.status,
        StatusCode::OK,
        "the save must succeed: {}",
        saved.body
    );
    assert!(
        path.exists(),
        "a save must land on the path the state names — it went somewhere else, which on a \
         developer's machine is `secrets/kessel-config.json`, their running daemon's own file"
    );

    // Confined, so it is now safe to store something and read it back.
    let saved = common::send(server::api::app(state.clone()), put(CONFIGURED)).await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.body);

    let stored = server::config::load(&path).expect("the state's own file holds the save");
    assert_eq!(
        stored.mqtt.as_ref().map(|mqtt| mqtt.host.as_str()),
        Some("10.0.0.9"),
        "the route wrote to the path its state names"
    );
    assert_eq!(stored.log_level.as_deref(), Some("trace"));

    // And the read comes back from the same place rather than from the ambient default.
    let read = common::get(server::api::app(state), "/api/config").await;
    assert!(
        read.body.contains("10.0.0.9"),
        "the read must come from the state's file too, or a save and a load disagree: {}",
        read.body
    );
}

#[test]
fn an_absent_config_file_is_not_an_error() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let loaded = server::config::load(&dir.path().join("nothing-here.json"))
        .expect("a missing config file is the ordinary first-run state, not a failure");
    assert!(
        loaded.mqtt.is_none() && loaded.log_level.is_none(),
        "a fresh install has nothing stored, and must start rather than refuse: {loaded:?}"
    );
}

#[test]
fn the_stored_file_is_owner_readable_only() {
    // The same protection the API token gets (`auth::write_owner_only`): this file holds a broker
    // credential.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("config.json");
        server::config::save(&path, &configured()).expect("saves");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "the config file holds a broker password and must be owner-only, like the token file"
        );
    }
}

/// The password is never returned, and a save that omits it does not wipe it.
///
/// One test, because a UI that cannot read the password back always omits it on save, so "never
/// returned" is only safe if "omitted means keep" holds.
#[test]
fn the_password_is_never_returned_and_omitting_it_keeps_it() {
    use server::config::{ConfigUpdate, view};

    let stored = configured();

    // 1. The view carries no password, in any form — not the value, not a masked stand-in.
    let rendered = format!("{:?}", view(&stored, |_| None));
    assert!(
        !rendered.contains("s3cret-do-not-print"),
        "the password must not reach the API view, even through Debug: {rendered}"
    );
    let json = serde_json::to_string(&view(&stored, |_| None)).expect("serialises");
    assert!(
        !json.contains("s3cret-do-not-print"),
        "nor the JSON body the browser receives: {json}"
    );
    assert!(
        json.contains(r#""mqtt_password_set":true"#),
        "but it must say that one is SET, or the UI cannot tell configured from unconfigured: {json}"
    );

    // 2. A save that omits the password keeps the stored one.
    let saved = ConfigUpdate {
        mqtt_host: Some("192.0.2.2".to_owned()),
        mqtt_port: Some(1884),
        mqtt_username: Some("kessel".to_owned()),
        mqtt_password: None,
        mqtt_base: None,
        mqtt_discovery: None,
        log_level: None,
        require_auth: None,
    }
    .apply(&stored);
    assert_eq!(
        saved
            .mqtt
            .as_ref()
            .and_then(|mqtt| mqtt.password.as_deref()),
        Some("s3cret-do-not-print"),
        "a save without a password must KEEP the stored one — otherwise every save from a form that \
         cannot read it back silently breaks broker authentication"
    );

    // 3. An explicit empty string clears it, so the operator is not stuck with a stored credential.
    let cleared = ConfigUpdate {
        mqtt_host: Some("192.0.2.2".to_owned()),
        mqtt_password: Some(String::new()),
        ..ConfigUpdate::default()
    }
    .apply(&stored);
    assert_eq!(
        cleared
            .mqtt
            .as_ref()
            .and_then(|mqtt| mqtt.password.as_deref()),
        None,
        "an explicit empty password clears it — the third of the three distinct states"
    );
}

/// An environment variable wins over the stored file, and the view says which ones.
///
/// A wrong host saved through the UI must not lock an operator out of fixing it, and the UI must be
/// able to say the daemon is ignoring what is stored.
#[test]
fn an_environment_variable_overrides_the_stored_file() {
    use server::config::view;
    use server::mqtt::MqttSettings;

    let stored = configured();
    let rendered = view(&stored, |key| {
        (key == MqttSettings::HOST_ENV).then(|| "192.0.2.9".to_owned())
    });

    assert_eq!(
        rendered.mqtt_host.as_deref(),
        Some("192.0.2.2"),
        "the view still reports what is STORED — that is what the form edits"
    );
    assert!(
        rendered
            .overridden_by_env
            .contains(&MqttSettings::HOST_ENV.to_owned()),
        "and it must name the variable that is overriding it, or the operator sees a host the daemon is \
         not using with nothing to explain why: {rendered:?}"
    );
    assert!(
        rendered.restart_required,
        "nothing here is hot-reloaded, and the UI must say so rather than imply the change is live"
    );
}

/// `PUT /api/config` is a mutation, so the token gate covers it, asserted on the real router.
///
/// `is_mutating` is keyed on the HTTP method, so this holds by construction; the route writes a
/// broker credential to disk, so it is checked anyway.
#[tokio::test]
async fn putting_a_config_requires_the_token() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use server::auth::ApiToken;
    use server::state::AppState;

    const TOKEN: &str = "test-token-for-the-config-route";

    let state = AppState::new(common::bus()).with_token(ApiToken::from_string(TOKEN));
    let body = r#"{"mqtt_host":null,"mqtt_port":null,"mqtt_username":null,"mqtt_password":null,"mqtt_base":null,"mqtt_discovery":null,"log_level":null}"#;

    let unauthenticated = Request::builder()
        .method("PUT")
        .uri("/api/config")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .expect("a well-formed request");
    let refused = common::send(server::api::app(state.clone()), unauthenticated).await;
    assert_eq!(
        refused.status,
        StatusCode::UNAUTHORIZED,
        "a config write without the token must be refused — it stores a broker credential: {}",
        refused.body
    );

    // The read is open, like every other read on this daemon, and safe to be: it carries no password.
    let read = common::get(server::api::app(state), "/api/config").await;
    assert_eq!(
        read.status,
        StatusCode::OK,
        "the config READ is open: {}",
        read.body
    );
    assert!(
        !read.body.contains(r#"password":""#),
        "and it carries no password value: {}",
        read.body
    );
}

// =================================================================================================
// Where the running daemon's broker actually came from.
// =================================================================================================

/// The view reports the **live** broker, not just the stored one.
///
/// Without this the settings page is a lie on a fresh add-on install: the stored host is empty, so the form
/// shows an empty host box, while the daemon is happily publishing to the broker the Supervisor handed it.
/// An operator looking at that concludes MQTT is off and types a host they did not need — which, by the
/// precedence rule, then *replaces* the working one.
#[test]
fn the_view_reports_the_broker_the_daemon_is_actually_using() {
    use server::config::{MqttSource, MqttStatus, view};

    let rendered = view(&StoredConfig::default(), |_| None).with_status(&MqttStatus {
        source: MqttSource::Supervisor,
        host: Some("core-mosquitto".to_owned()),
        port: Some(1883),
    });

    assert_eq!(
        rendered.mqtt_host, None,
        "nothing is stored, and it says so"
    );
    assert_eq!(rendered.mqtt.source, MqttSource::Supervisor);
    assert_eq!(rendered.mqtt.host.as_deref(), Some("core-mosquitto"));
    assert_eq!(rendered.mqtt.port, Some(1883));
}

/// A daemon that found no broker says `unconfigured`, and that is the default a test state carries.
#[test]
fn no_broker_is_a_named_state_rather_than_an_absence() {
    use server::config::{MqttSource, view};

    let rendered = view(&StoredConfig::default(), |_| None);

    assert_eq!(
        rendered.mqtt.source,
        MqttSource::Unconfigured,
        "the UI has to distinguish `no broker` from `a broker whose source I could not determine`"
    );
    assert_eq!(rendered.mqtt.host, None);
    assert!(
        rendered.mqtt.autodiscovery,
        "autodiscovery is on unless the operator turned it off — the view reports the option, and the \
         form's copy depends on it"
    );
}

/// `GET /api/config` carries the live status out of the state, not a recomputed guess.
#[tokio::test]
async fn the_config_route_reports_the_live_broker_source() {
    use server::config::{MqttSource, MqttStatus};
    use server::state::AppState;

    let state = AppState::new(common::bus()).with_mqtt_status(MqttStatus {
        source: MqttSource::Supervisor,
        host: Some("core-mosquitto".to_owned()),
        port: Some(1883),
    });

    let read = common::get(server::api::app(state), "/api/config").await;

    assert!(
        read.body.contains("\"source\":\"supervisor\""),
        "the route must report where the RUNNING daemon's broker came from: {}",
        read.body
    );
    assert!(
        read.body.contains("core-mosquitto"),
        "and which broker that is: {}",
        read.body
    );
}
