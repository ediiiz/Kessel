//! What the API tells the browser about **names**.
//!
//! `server::labels` resolves a display name from the contributed tables; these tests assert the
//! resolution actually reaches the wire, and — the half that matters more — that it never replaces
//! the raw identifiers a write depends on.
//!
//! The brand and `vib` assertions read their expected values back out of the fixture profile rather
//! than hardcoding them, the same discipline `tests/common/mod.rs` uses for the `haId`: a fixture is
//! re-scrubbed from time to time and a hardcoded model number would pin the scrubber, not the API.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "tests may panic on failed assertions; indexing a JSON response is how a test \
                  asserts its shape — a missing key must panic loudly, not be silently `None`"
    )
)]

mod common;

use axum::http::StatusCode;
use hc_core::device::DeviceRole;
use hc_profile::Profile;
use serde_json::Value;
use server::state::{AppState, DeviceEntry};

/// The scrubbed hob fixture, onboarded under the slug `cooktop`.
fn state_with_hob() -> AppState {
    let state = AppState::new(common::bus());
    let profile = Profile::from_zip_bytes(&common::hob_zip_bytes()).expect("the fixture parses");
    state.devices().insert(
        "cooktop",
        DeviceEntry::from_profile("cooktop", DeviceRole::Cooktop, &profile),
    );
    state
}

/// The fixture's own brand and `vib`, so the assertions below hardcode neither.
fn fixture_brand_and_vib() -> (String, String) {
    let profile = Profile::from_zip_bytes(&common::hob_zip_bytes()).expect("the fixture parses");
    let identity = profile.identity();
    (identity.brand.clone(), identity.vib.clone())
}

/// `GET /api/devices` reports the brand and the model designation.
///
/// `crate::labels` keys its contributed tables on exactly this pair, so a contributor needs them
/// to know which file to write.
#[tokio::test]
async fn the_summary_names_the_model() {
    let (brand, vib) = fixture_brand_and_vib();
    assert!(
        !brand.is_empty() && !vib.is_empty(),
        "the fixture must carry a brand and a vib for this test to mean anything"
    );

    let reply = common::get(server::api::app(state_with_hob()), "/api/devices").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let device = &reply.json()[0];

    assert_eq!(device["brand"], Value::String(brand));
    assert_eq!(device["vib"], Value::String(vib));
    // The slug and role class are unchanged: this widened the summary, it did not repurpose it.
    assert_eq!(device["slug"], "cooktop");
    assert_eq!(device["role"], "cooktop");
}

/// Every descriptor carries a resolved name, and the shared prefix layer is actually consulted.
#[tokio::test]
async fn entities_carry_resolved_labels() {
    let reply = common::get(
        server::api::app(state_with_hob()),
        "/api/devices/cooktop/entities",
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    let entities = reply.json();
    let entities = entities.as_array().expect("a list of descriptors");
    assert!(!entities.is_empty(), "the hob fixture exposes features");

    // A label is never absent and never blank: the derived layer cannot fail, so the UI needs no
    // fallback of its own.
    for entity in entities {
        let label = entity["label"].as_str().expect("every entity has a label");
        assert!(
            !label.trim().is_empty(),
            "a blank label reached the wire for {:?}",
            entity["feature_key"]
        );
        assert!(
            matches!(
                entity["label_source"].as_str(),
                Some("model" | "prefix" | "derived")
            ),
            "unexpected label_source {:?}",
            entity["label_source"]
        );
    }

    // At least one `BSH.Common` key must resolve from the contributed prefix file — otherwise the
    // layering is wired but inert, which is the failure this test exists to catch.
    assert!(
        entities
            .iter()
            .any(|entity| entity["label_source"] == "prefix"),
        "no entity resolved from the prefix layer; data/labels/prefixes/BSH.Common.toml names keys \
         every appliance has, so the lookup is not reaching it"
    );
}

/// A friendly name never displaces the raw identifiers.
///
/// This is rule 1 of `server::labels` expressed as a test, and it is the one that keeps a contributed
/// file from being able to affect an appliance: `options` stays the appliance's own declared members,
/// `option_labels` is a separate map beside it, and `value` still matches a member of `options`.
#[tokio::test]
async fn raw_identifiers_survive_beside_their_labels() {
    let reply = common::get(
        server::api::app(state_with_hob()),
        "/api/devices/cooktop/entities",
    )
    .await;
    let entities = reply.json();

    for entity in entities.as_array().expect("a list of descriptors") {
        // The dotted key is still there, untouched, for the operator and for the MQTT topic.
        if let Some(key) = entity["feature_key"].as_str() {
            assert!(
                key.contains('.'),
                "feature_key was rewritten into a display name: {key}"
            );
        }

        let options: Vec<&str> = entity["options"]
            .as_array()
            .expect("options is always a list")
            .iter()
            .map(|option| option.as_str().expect("a member name"))
            .collect();

        // Members are raw. A contributed name in here would be written to the appliance verbatim.
        for option in &options {
            assert!(
                !option.contains(' '),
                "a display name leaked into `options`, which is what a write carries: {option}"
            );
        }

        // Every label key names a member that exists — a stale contributed entry cannot invent one.
        let labels = entity["option_labels"]
            .as_object()
            .expect("option_labels is always a map");
        for member in labels.keys() {
            assert!(
                options.contains(&member.as_str()),
                "option_labels names {member:?}, which is not a declared member of {:?}",
                entity["feature_key"]
            );
        }
    }
}
