//! `GET /api/devices/{slug}/programs`: the appliance's programmes, named.
//!
//! The route is a read. There is no start control and no companion write route:
//! `hc_core::bridge::validate_write` would accept `/ro/selectedProgram`, so a start affordance here
//! could begin a programme on a cooktop or an oven. `programs_are_a_read_only_surface` pins the
//! absence.

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

/// The token `programs_are_a_read_only_surface` authorises with; a test value, as in
/// `tests/token_auth.rs`.
const TEST_TOKEN: &str = "test-token-not-a-real-one";

/// A state carrying the scrubbed dishwasher under the slug `dishwasher`.
///
/// The dishwasher because the contributed prefix file names its programmes, so the label lookup is
/// exercised too.
fn state_with_dishwasher() -> AppState {
    let state = AppState::new(common::bus());
    let profile = Profile::from_zip_bytes(&common::fixture_zip_bytes("dishwasher-tls.zip"))
        .expect("the fixture parses");
    state.devices().insert(
        "dishwasher",
        DeviceEntry::from_profile("dishwasher", DeviceRole::Dishwasher, &profile),
    );
    state
}

/// The route lists the appliance's programmes, each with a resolved name and its raw key.
#[tokio::test]
async fn programs_are_listed_and_named() {
    let reply = common::get(
        server::api::app(state_with_dishwasher()),
        "/api/devices/dishwasher/programs",
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);

    let programs = reply.json();
    let programs = programs.as_array().expect("a list of programmes");
    assert!(
        !programs.is_empty(),
        "the dishwasher fixture declares programmes; an empty list means \
         DeviceMapping::programs() is still not being read"
    );

    for program in programs {
        // Named, never blank — the derived layer guarantees this much.
        let label = program["label"]
            .as_str()
            .expect("every programme has a label");
        assert!(
            !label.trim().is_empty(),
            "a blank programme label: {program}"
        );

        // The raw key survives beside the name, same as for features.
        if let Some(key) = program["feature_key"].as_str() {
            assert!(
                key.contains('.'),
                "feature_key was rewritten into a display name: {key}"
            );
        }

        assert!(
            matches!(
                program["label_source"].as_str(),
                Some("model" | "prefix" | "derived")
            ),
            "unexpected label_source {:?}",
            program["label_source"]
        );

        // `execution` is the appliance's own word, passed through unchanged so a future start
        // control can gate on it.
        assert!(
            program["execution"].is_string() || program["execution"].is_null(),
            "execution must be the appliance's verbatim word or absent: {:?}",
            program["execution"]
        );
    }

    // The contributed dishwasher prefix file names ten programmes, so at least one must resolve above
    // the derived layer — otherwise the lookup is wired but inert.
    assert!(
        programs
            .iter()
            .any(|program| program["label_source"] == "prefix"),
        "no programme resolved from data/labels/prefixes/Dishcare.Dishwasher.toml"
    );
}

/// An unknown slug is a 404; a known device with no programmes is an empty list.
///
/// The UI has to be able to tell the two apart. The empty case uses the fridge: of the four
/// scrubbed fixtures it is the only one that declares no programmes (the hob declares 73).
#[tokio::test]
async fn an_unknown_device_is_not_an_empty_program_list() {
    let missing = common::get(
        server::api::app(state_with_dishwasher()),
        "/api/devices/nosuchdevice/programs",
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND, "{}", missing.body);

    let state = AppState::new(common::bus());
    let profile = Profile::from_zip_bytes(&common::fixture_zip_bytes("fridge-tls.zip"))
        .expect("the fixture parses");
    state.devices().insert(
        "fridgefreezer",
        DeviceEntry::from_profile("fridgefreezer", DeviceRole::Fridge, &profile),
    );
    let fridge = common::get(
        server::api::app(state),
        "/api/devices/fridgefreezer/programs",
    )
    .await;
    assert_eq!(fridge.status, StatusCode::OK, "{}", fridge.body);
    assert!(
        fridge.json().as_array().expect("a list").is_empty(),
        "the fridge declares no programmes, and an existing device must still answer 200"
    );
}

/// Listing programmes added no way to start one.
///
/// `hc_core` would accept a `/ro/selectedProgram` frame, so the only thing keeping a dashboard click
/// from starting a cooktop is that no such route exists.
///
/// The token is presented so the request gets past the mutation gate (which would answer 401); a
/// 405 then shows there is no handler at all.
#[tokio::test]
async fn programs_are_a_read_only_surface() {
    let state = state_with_dishwasher().with_token(ApiToken::from_string(TEST_TOKEN));
    let request = Request::builder()
        .method("POST")
        .uri("/api/devices/dishwasher/programs")
        .header("content-type", "application/json")
        .header(TOKEN_HEADER, TEST_TOKEN)
        .body(Body::from(r#"{"uid":1}"#))
        .expect("a valid request");
    let posted = common::send(server::api::app(state), request).await;
    assert_eq!(
        posted.status,
        StatusCode::METHOD_NOT_ALLOWED,
        "an authorised POST must still find no handler — listing programmes must not have added a \
         way to start one: {}",
        posted.body
    );
}
