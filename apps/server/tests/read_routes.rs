//! Read routes are open without a token.
//!
//! The token gates writes, not reads. Gating reads too would make the dashboard and the frame
//! inspector unusable until the operator found the token file. Reads are open because the socket
//! is not published beyond the container network (see `loopback_bind.rs`). A read carrying no
//! token header at all succeeds, which is what the SPA does on first paint.

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

use axum::http::StatusCode;
use hc_core::device::DeviceRole;
use hc_profile::Profile;
use server::state::{AppState, DeviceEntry};

/// A state carrying one onboarded device: the scrubbed hob fixture, under the slug `cooktop`.
fn state_with_hob() -> AppState {
    let state = AppState::new(common::bus());
    let profile = Profile::from_zip_bytes(&common::hob_zip_bytes()).expect("the fixture parses");
    state.devices().insert(
        "cooktop",
        DeviceEntry::from_profile("cooktop", DeviceRole::Cooktop, &profile),
    );
    state
}

#[tokio::test]
async fn read_routes_are_open_on_loopback() {
    let state = state_with_hob();

    let health = common::get(server::api::app(state.clone()), "/api/health").await;
    assert_eq!(
        health.status,
        StatusCode::OK,
        "GET /api/health must not require a token: {}",
        health.body
    );
    assert_eq!(
        health.json()["device_count"],
        1,
        "health reports the registry's device count"
    );

    let devices = common::get(server::api::app(state.clone()), "/api/devices").await;
    assert_eq!(
        devices.status,
        StatusCode::OK,
        "GET /api/devices must not require a token: {}",
        devices.body
    );
    let listed = devices.json();
    assert_eq!(
        listed[0]["slug"], "cooktop",
        "the device list is keyed by the non-identifier slug: {listed}"
    );
    assert_eq!(
        listed[0]["role"], "cooktop",
        "the device is described by its role class, never its identity: {listed}"
    );
    assert_eq!(
        listed[0]["ever_writable"], true,
        "every appliance is writable, and the dashboard must be told so: {listed}"
    );

    let entities = common::get(
        server::api::app(state.clone()),
        "/api/devices/cooktop/entities",
    )
    .await;
    assert_eq!(
        entities.status,
        StatusCode::OK,
        "GET /api/devices/{{slug}}/entities must not require a token: {}",
        entities.body
    );
    let features = entities.json();
    assert!(
        features.as_array().is_some_and(|list| list.len() > 20),
        "coverage is total: the hob profile has hundreds of features, got {}",
        features.as_array().map_or(0, Vec::len)
    );

    // An unknown slug is a 404, not a 500 or an empty 200, so the UI can tell "no such device" from
    // "a device with no features".
    let missing = common::get(server::api::app(state), "/api/devices/nope/entities").await;
    assert_eq!(
        missing.status,
        StatusCode::NOT_FOUND,
        "an unknown device slug is 404: {}",
        missing.body
    );
}

/// An offline device must say **why**, not just that it is.
///
/// `offline` alone cannot tell a powered-off appliance from one that is answering every dial and
/// rejecting the handshake, and the two want opposite responses — the second will not fix itself.
/// The hob spent an evening in the second state while the dashboard showed the first; the reason was
/// on the trace bus the whole time, at `debug` level, where nobody looks.
#[tokio::test]
async fn an_offline_device_reports_why() {
    // Host-redacted at construction, which is the shape `crate::live_transport` produces.
    const REASON: &str =
        "device offline: the connect or handshake failed: transport closed / no more frames";

    let state = state_with_hob();
    state.devices().set_offline("cooktop", REASON);

    let devices = common::get(server::api::app(state), "/api/devices").await;
    let summary = &devices.json()[0];

    assert_eq!(
        summary["connection_state"], "offline",
        "the state is still offline: the reason accompanies it, it does not replace it"
    );
    assert_eq!(
        summary["last_failure"], REASON,
        "an operator must be able to see why a device is offline without reading the frame \
         inspector: {}",
        devices.body
    );
}
