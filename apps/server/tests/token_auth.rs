//! Mutating routes require the API token.
//!
//! * a mutating request with no token is refused with 401 rather than 404, so the gate is visible
//!   to the operator;
//! * a mutating request with the wrong token is refused;
//! * a mutating request with the right token gets past the gate and then meets the write path,
//!   which is a separate decision;
//! * the gate is keyed on the method, not on a route list, so a route added later is gated by
//!   default.
//!
//! The token travels in the `X-Kessel-Token` request header. A cross-site HTML `<form>` can POST to
//! this daemon but cannot set a custom header, which is what makes it CSRF-safe.
//!
//! No socket is opened: the device is onboarded but has no live session.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "tests may panic on failed assertions; indexing a JSON response panics loudly \
                  on a missing key"
    )
)]

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hc_core::device::DeviceRole;
use hc_profile::Profile;
use server::auth::{ApiToken, TOKEN_HEADER};
use server::state::{AppState, DeviceEntry};

/// The token this test authorises with. Not a secret and not a real one: the production token is
/// generated at first run (`ApiToken::load_or_create`).
const TEST_TOKEN: &str = "test-token-not-a-real-one";

/// One onboarded device (the scrubbed hob fixture) and a known token.
fn state() -> AppState {
    let state = AppState::new(common::bus()).with_token(ApiToken::from_string(TEST_TOKEN));
    let profile = Profile::from_zip_bytes(&common::hob_zip_bytes()).expect("the fixture parses");
    state.devices().insert(
        "cooktop",
        DeviceEntry::from_profile("cooktop", DeviceRole::Cooktop, &profile),
    );
    state
}

/// A `POST` with a JSON body and an optional token header.
fn post(uri: &str, token: Option<&str>, body: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header(TOKEN_HEADER, token);
    }
    builder
        .body(Body::from(body.to_owned()))
        .expect("a well-formed request")
}

#[tokio::test]
async fn mutating_routes_require_token() {
    let write_body = r#"{"feature_key":"BSH.Common.Setting.PowerState","value":"On"}"#;

    // 1. No token at all.
    let anonymous = common::send(
        server::api::app(state()),
        post("/api/devices/cooktop/write", None, write_body),
    )
    .await;
    assert_eq!(
        anonymous.status,
        StatusCode::UNAUTHORIZED,
        "a mutating request with no token must be 401: {}",
        anonymous.body
    );
    assert!(
        anonymous.json()["message"].is_string(),
        "the refusal carries an ApiError body the UI can render: {}",
        anonymous.body
    );

    // 2. The wrong token.
    let wrong = common::send(
        server::api::app(state()),
        post("/api/devices/cooktop/write", Some("nope"), write_body),
    )
    .await;
    assert_eq!(
        wrong.status,
        StatusCode::UNAUTHORIZED,
        "a mutating request with the wrong token must be 401: {}",
        wrong.body
    );

    // 3. The right token gets past the auth gate and meets the write path, which refuses: there is
    // no live session. Two different decisions, visibly distinct.
    let authorised = common::send(
        server::api::app(state()),
        post("/api/devices/cooktop/write", Some(TEST_TOKEN), write_body),
    )
    .await;
    assert_ne!(
        authorised.status,
        StatusCode::UNAUTHORIZED,
        "the correct token must pass the auth gate: {}",
        authorised.body
    );
    assert_eq!(
        authorised.status,
        StatusCode::FORBIDDEN,
        "an authorised hob write is refused by the write guard, not by auth: {}",
        authorised.body
    );
    assert_eq!(
        authorised.json()["outcome"],
        "refused",
        "the refusal is reported as a WriteOutcome: {}",
        authorised.body
    );

    // 4. The gate is method-keyed, so every mutating route is covered, including onboarding.
    let onboarding = common::send(
        server::api::app(state()),
        post("/api/onboarding/commit?slug=cooktop", None, ""),
    )
    .await;
    assert_eq!(
        onboarding.status,
        StatusCode::UNAUTHORIZED,
        "onboarding is a mutating route and is gated by the same middleware: {}",
        onboarding.body
    );

    // 5. A read is still open, with no token, on the same router.
    let read = common::get(server::api::app(state()), "/api/devices").await;
    assert_eq!(
        read.status,
        StatusCode::OK,
        "the mutation gate must not have closed the read routes: {}",
        read.body
    );
}
