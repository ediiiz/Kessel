//! Authentication is a setting, and it is off by default.
//!
//! Behind Home Assistant's ingress the browser is already authenticated, and the add-on publishes no
//! port (`loopback_bind.rs` keeps that true), so `require_auth` is off unless asked for. This file
//! pins:
//!
//! 1. off, a mutating request with no credential reaches its handler;
//! 2. off, `GET /api/session` says so, and the SPA renders no sign-in control;
//! 3. `AppState`'s default is the gated one, which `token_auth.rs` and `session_cookie.rs` test;
//! 4. a save that does not mention `require_auth` does not switch it off.
//!
//! The only mutating route exercised deletes a slug that was never onboarded, which reaches no
//! transport.

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

use axum::body::Body;
use axum::http::{Request, StatusCode};
use server::config::{ConfigUpdate, StoredConfig};
use server::state::AppState;

/// A mutating request with no token and no cookie. `DELETE` of a device that was never onboarded: it is
/// mutating by method — which is what the gate keys on — and its handler answers 404, so the two
/// outcomes are distinguishable at a glance.
fn anonymous_mutation() -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri("/api/devices/never-onboarded")
        .body(Body::empty())
        .expect("a well-formed request")
}

#[tokio::test]
async fn with_authentication_off_a_mutating_request_needs_no_credential() {
    let state = AppState::new(common::bus()).with_auth_required(false);

    let reply = common::send(server::api::app(state), anonymous_mutation()).await;

    assert_eq!(
        reply.status,
        StatusCode::NOT_FOUND,
        "the request must reach its handler — a 401 here means the gate is still refusing an \
         unauthenticated caller on a daemon configured not to authenticate: {}",
        reply.body
    );
}

#[tokio::test]
async fn with_authentication_on_the_same_request_is_refused() {
    // No `with_auth_required` call: `AppState::new` is gated, which is what every other test in this
    // suite relies on, and this asserts that rather than leaving it to be discovered.
    let state = AppState::new(common::bus());

    let reply = common::send(server::api::app(state), anonymous_mutation()).await;

    assert_eq!(
        reply.status,
        StatusCode::UNAUTHORIZED,
        "the gate is on by default in `AppState::new`, and `main` is the one place that turns it \
         off: {}",
        reply.body
    );
}

/// The SPA asks this on load and renders the sign-in control from the answer.
#[tokio::test]
async fn the_session_route_reports_that_nothing_needs_signing_in_to() {
    let state = AppState::new(common::bus()).with_auth_required(false);

    let reply = common::get(server::api::app(state), "/api/session").await;

    assert_eq!(
        reply.json(),
        serde_json::json!({ "authenticated": true, "auth_required": false }),
        "`authenticated` answers \"may this caller mutate\", and with the gate off it may — a `false` \
         here would make the SPA render a sign-in form for a daemon with nothing to sign in to"
    );
}

/// The stored default, read through the same function the daemon calls.
///
/// No environment manipulation: `std::env::set_var` is unsafe in the 2024 edition and racy across
/// libtest's threads. The environment arm is covered by `parse_require_auth`'s unit tests in
/// `server::auth`.
#[test]
fn an_unset_setting_means_no_authentication() {
    assert_eq!(
        StoredConfig::default().require_auth,
        None,
        "a fresh install stores nothing, and nothing must mean off"
    );

    let rendered = server::config::view(&StoredConfig::default(), |_| None);
    assert!(
        !rendered.require_auth,
        "and the UI must show the switch off rather than defaulting its checkbox to on: {rendered:?}"
    );
}

#[test]
fn the_view_names_the_environment_variable_only_when_it_is_understood() {
    let rendered = server::config::view(&StoredConfig::default(), |key| {
        (key == server::auth::REQUIRE_AUTH_ENV).then(|| "true".to_owned())
    });
    assert!(
        rendered
            .overridden_by_env
            .contains(&server::auth::REQUIRE_AUTH_ENV.to_owned()),
        "an understood value is in force, and the operator has to be told the stored switch is being \
         ignored: {rendered:?}"
    );

    let typo = server::config::view(&StoredConfig::default(), |key| {
        (key == server::auth::REQUIRE_AUTH_ENV).then(|| "yes-please".to_owned())
    });
    assert!(
        typo.overridden_by_env.is_empty(),
        "an unrecognised value is ignored by the daemon, so the UI must not claim it is overriding \
         anything — that would send an operator looking for a setting that is not in force: {typo:?}"
    );
}

/// The one direction that must not happen by accident.
#[test]
fn a_save_that_omits_the_setting_keeps_it_on() {
    let stored = StoredConfig {
        require_auth: Some(true),
        ..StoredConfig::default()
    };

    let saved = ConfigUpdate {
        mqtt_host: Some("192.0.2.2".to_owned()),
        ..ConfigUpdate::default()
    }
    .apply(&stored);

    assert_eq!(
        saved.require_auth,
        Some(true),
        "absent means keep. Every other field here treats `null` as \"unset it\", and for this one \
         that would let a client saving an unrelated MQTT change switch the gate off"
    );

    let switched_off = ConfigUpdate {
        require_auth: Some(false),
        ..ConfigUpdate::default()
    }
    .apply(&stored);
    assert_eq!(
        switched_off.require_auth,
        Some(false),
        "and an explicit `false` still turns it off — keep-semantics must not make it unremovable"
    );
}
