//! The two-phase onboarding flow: preview, then commit.
//!
//! * `POST /api/onboarding/preview` parses only. It registers, persists and dials nothing, and
//!   reports what the archive says so the operator can correct it.
//! * `POST /api/onboarding/commit?slug=&host=&port=` carries the operator's confirmed choices with
//!   the same ZIP re-posted. It registers, persists, dials, and launches the device task.
//!
//! The ZIP is uploaded twice rather than staged server-side, so there is no staging expiry for a
//! confirmation dialog left open to run into.
//!
//! The preview does not report the derived host. `hc_profile::DeviceIdentity::derived_host` contains
//! the `haId` on both transports (bare on AES, `brand-type-haId` on TLS), so it must not reach an
//! API response or the DOM. It also rarely resolves, since the add-on has no `host_network` for mDNS,
//! so the host field starts empty.
//!
//! No appliance is contacted: every test injects a recording fake through `ConnectionTester` and a
//! `TempDir` through `ProfileStore`.

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

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use parking_lot::Mutex;
use server::auth::{ApiToken, TOKEN_HEADER};
use server::onboarding::{ConnectionTarget, ConnectionTester};
use server::state::AppState;
use server::store::ProfileStore;

const TEST_TOKEN: &str = "test-token-not-a-real-one";

/// Records what it was asked to dial; opens nothing. Every host "succeeds" unless `fail` is set.
#[derive(Debug, Default)]
struct Recorder {
    dialled: Mutex<Vec<(String, Option<u16>)>>,
    fail: bool,
}

impl Recorder {
    fn ok() -> Self {
        Self::default()
    }

    const fn failing() -> Self {
        Self {
            dialled: Mutex::new(Vec::new()),
            fail: true,
        }
    }

    fn dialled(&self) -> Vec<(String, Option<u16>)> {
        self.dialled.lock().clone()
    }
}

#[async_trait]
impl ConnectionTester for Recorder {
    async fn test(&self, target: &ConnectionTarget) -> Result<(), String> {
        self.dialled.lock().push((target.host.clone(), target.port));
        if self.fail {
            Err("no route to host (fake)".to_owned())
        } else {
            Ok(())
        }
    }
}

/// A state with a known token, a recording tester and a store in `dir`.
fn state(tester: Arc<Recorder>, dir: &std::path::Path) -> AppState {
    AppState::new(common::bus())
        .with_token(ApiToken::from_string(TEST_TOKEN))
        .with_connection_tester(tester)
        .with_profile_store(ProfileStore::new(dir))
}

/// `POST /api/onboarding/preview` with a fixture's bytes.
fn post_preview(fixture: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/onboarding/preview")
        .header("content-type", "application/zip")
        .header(TOKEN_HEADER, TEST_TOKEN)
        .body(Body::from(common::fixture_zip_bytes(fixture)))
        .expect("a well-formed request")
}

/// `POST /api/onboarding/commit` with the operator's confirmed choices in the query string.
fn post_commit(fixture: &str, query: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/api/onboarding/commit?{query}"))
        .header("content-type", "application/zip")
        .header(TOKEN_HEADER, TEST_TOKEN)
        .body(Body::from(common::fixture_zip_bytes(fixture)))
        .expect("a well-formed request")
}

// ---------------------------------------------------------------------------------------------
// Phase 1: preview.
// ---------------------------------------------------------------------------------------------

/// A preview reports what the archive says and dials nothing, so the operator can correct the
/// address before a ~20 s connection attempt is spent on the wrong one.
#[tokio::test]
async fn a_preview_parses_without_dialling_or_registering() {
    let tester = Arc::new(Recorder::ok());
    let dir = tempfile::tempdir().unwrap();
    let state = state(Arc::clone(&tester), dir.path());

    let reply = common::send(server::api::app(state.clone()), post_preview("hob-aes.zip")).await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();

    assert_eq!(
        body["role"], "cooktop",
        "the role class the manifest declares"
    );
    assert_eq!(
        body["suggested_slug"], "cooktop",
        "the slug the daemon would allocate, offered as the dialog's default"
    );
    assert!(
        body["existing_slug"].is_null(),
        "nothing is onboarded yet, so this is not an update"
    );
    assert!(
        body["entity_count"].as_u64().unwrap() > 0,
        "the feature model is populated: coverage is total"
    );
    assert_eq!(
        body["transport"], "aes",
        "the hob's key material selects AES"
    );
    assert_eq!(
        body["default_port"], 80,
        "the port field is prefilled with what the transport would have used anyway"
    );

    assert!(
        tester.dialled().is_empty(),
        "a preview must not dial: correcting the address is the whole reason this phase exists"
    );
    assert!(
        state.devices().is_empty(),
        "a preview must not register the device — nothing is committed until the operator confirms"
    );
    assert_eq!(
        std::fs::read_dir(dir.path()).map_or(0, Iterator::count),
        0,
        "and it must not persist anything"
    );
}

/// The preview carries no host field at all.
///
/// `derived_host()` contains the `haId` on both transports, so prefilling it would leak an appliance
/// identifier into an HTTP response. The assertion is on the absence of the key.
#[tokio::test]
async fn a_preview_never_reports_a_derived_host() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(Arc::new(Recorder::ok()), dir.path());

    for fixture in [
        "hob-aes.zip",
        "oven-tls.zip",
        "dishwasher-tls.zip",
        "fridge-tls.zip",
    ] {
        let reply = common::send(server::api::app(state.clone()), post_preview(fixture)).await;
        let body = reply.json();
        let object = body.as_object().expect("the preview is a JSON object");
        for forbidden in ["host", "derived_host", "ha_id", "haId", "serial", "mac"] {
            assert!(
                !object.contains_key(forbidden),
                "{fixture}'s preview must carry no {forbidden:?} key: the derived host is haId-bearing \
                 on both transports"
            );
        }

        // No value anywhere in the body may contain the manifest's haId.
        let rendered = body.to_string();
        let ha_id = common::fixture_ha_id(fixture);
        assert!(
            !rendered.contains(&ha_id),
            "{fixture}'s preview body must not contain its haId anywhere"
        );
    }
}

/// A ZIP that is not a profile is a 400, on the preview route too.
#[tokio::test]
async fn a_preview_of_junk_is_a_bad_request() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(Arc::new(Recorder::ok()), dir.path());
    let request = Request::builder()
        .method("POST")
        .uri("/api/onboarding/preview")
        .header(TOKEN_HEADER, TEST_TOKEN)
        .body(Body::from(&b"not a zip"[..]))
        .unwrap();

    let reply = common::send(server::api::app(state), request).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------------------------
// Phase 2: commit.
// ---------------------------------------------------------------------------------------------

/// A commit dials the operator's host and port, registers the device and persists it.
#[tokio::test]
async fn a_commit_dials_the_operators_choices_and_persists() {
    let tester = Arc::new(Recorder::ok());
    let dir = tempfile::tempdir().unwrap();
    let state = state(Arc::clone(&tester), dir.path());

    let reply = common::send(
        server::api::app(state.clone()),
        post_commit("hob-aes.zip", "slug=kitchen-hob&host=10.0.0.42&port=8080"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();

    assert_eq!(
        body["device"]["slug"], "kitchen-hob",
        "the operator's chosen slug, not the daemon's suggestion"
    );
    assert_eq!(body["connection"]["status"], "connected");
    assert_eq!(body["host_source"], "manual");

    assert_eq!(
        tester.dialled(),
        vec![("10.0.0.42".to_owned(), Some(8080))],
        "exactly the host and port the operator confirmed — the port must reach the dial, not just \
         the record"
    );

    let stored = ProfileStore::new(dir.path()).load_all();
    assert_eq!(stored.len(), 1, "the device is on disk");
    let device = stored.first().unwrap();
    assert_eq!(device.slug, "kitchen-hob");
    assert_eq!(device.record.manual_host.as_deref(), Some("10.0.0.42"));
    assert_eq!(device.record.port, Some(8080));
}

/// A failed connection test still persists the device.
///
/// An appliance that is merely powered off must not cost the operator their upload; the verdict is
/// reported and the device can be corrected afterwards.
#[tokio::test]
async fn a_failed_connection_test_still_persists_the_device() {
    let tester = Arc::new(Recorder::failing());
    let dir = tempfile::tempdir().unwrap();
    let state = state(Arc::clone(&tester), dir.path());

    let reply = common::send(
        server::api::app(state.clone()),
        post_commit("hob-aes.zip", "slug=cooktop&host=10.0.0.42"),
    )
    .await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "a refusal is a result, not an error"
    );
    let body = reply.json();
    assert_eq!(body["connection"]["status"], "failed");
    assert_eq!(
        body["connection"]["needs_manual_ip"], true,
        "the manual-IP escalation still applies"
    );

    assert_eq!(
        ProfileStore::new(dir.path()).load_all().len(),
        1,
        "a powered-off appliance must not cost the operator the upload"
    );
    assert!(
        state.devices().contains("cooktop"),
        "and it is registered, so the dashboard can offer a correction"
    );
}

/// An absent port override means the transport's default reaches the dial as `None`.
#[tokio::test]
async fn an_absent_port_is_the_transport_default() {
    let tester = Arc::new(Recorder::ok());
    let dir = tempfile::tempdir().unwrap();
    let state = state(Arc::clone(&tester), dir.path());

    common::send(
        server::api::app(state),
        post_commit("hob-aes.zip", "slug=cooktop&host=10.0.0.42"),
    )
    .await;

    assert_eq!(tester.dialled(), vec![("10.0.0.42".to_owned(), None)]);
    assert_eq!(
        ProfileStore::new(dir.path())
            .load_all()
            .first()
            .unwrap()
            .record
            .port,
        None,
        "an absent override is stored as absent, not as a hardcoded 80"
    );
}

/// A commit with no host falls back to the profile-derived one, and says so.
///
/// The UI always sends a host, but the API does not require one for an operator whose DNS resolves.
#[tokio::test]
async fn a_commit_without_a_host_uses_the_derived_one() {
    let tester = Arc::new(Recorder::ok());
    let dir = tempfile::tempdir().unwrap();
    let state = state(Arc::clone(&tester), dir.path());

    let reply = common::send(
        server::api::app(state),
        post_commit("hob-aes.zip", "slug=cooktop"),
    )
    .await;
    assert_eq!(reply.json()["host_source"], "derived");
    assert_eq!(
        tester.dialled().len(),
        1,
        "something was dialled — the derived host, which this test deliberately does not name"
    );
    assert_eq!(
        ProfileStore::new(dir.path())
            .load_all()
            .first()
            .unwrap()
            .record
            .manual_host,
        None,
        "no override is stored, so the host stays derived across a restart too"
    );
}

// ---------------------------------------------------------------------------------------------
// The upsert.
// ---------------------------------------------------------------------------------------------

/// Re-committing the same appliance updates it in place: one device, not two.
///
/// Matched on the fingerprint, so a re-exported profile updates the device rather than creating
/// `cooktop-2`.
#[tokio::test]
async fn recommitting_the_same_appliance_updates_it() {
    let tester = Arc::new(Recorder::ok());
    let dir = tempfile::tempdir().unwrap();
    let state = state(Arc::clone(&tester), dir.path());

    common::send(
        server::api::app(state.clone()),
        post_commit("hob-aes.zip", "slug=cooktop&host=10.0.0.1"),
    )
    .await;
    common::send(
        server::api::app(state.clone()),
        post_commit("hob-aes.zip", "slug=cooktop&host=10.0.0.2&port=9000"),
    )
    .await;

    assert_eq!(state.devices().len(), 1, "one device, updated");
    let stored = ProfileStore::new(dir.path()).load_all();
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored.first().unwrap().record.manual_host.as_deref(),
        Some("10.0.0.2"),
        "the second commit's host wins"
    );
    assert_eq!(stored.first().unwrap().record.port, Some(9000));
}

/// A preview of an appliance already onboarded reports the slug it would update.
///
/// This lets the dialog say "this will update `cooktop`".
#[tokio::test]
async fn a_preview_reports_the_slug_it_would_update() {
    let tester = Arc::new(Recorder::ok());
    let dir = tempfile::tempdir().unwrap();
    let state = state(Arc::clone(&tester), dir.path());

    common::send(
        server::api::app(state.clone()),
        post_commit("hob-aes.zip", "slug=kitchen-hob&host=10.0.0.1"),
    )
    .await;

    let body = common::send(server::api::app(state), post_preview("hob-aes.zip"))
        .await
        .json();
    assert_eq!(
        body["existing_slug"], "kitchen-hob",
        "the same appliance is recognised and its current slug reported"
    );
    assert_eq!(
        body["suggested_slug"], "kitchen-hob",
        "and the dialog defaults to updating it rather than to a fresh name"
    );
}

/// Re-committing the same appliance under a new slug renames it rather than duplicating it.
#[tokio::test]
async fn recommitting_under_a_new_slug_renames_the_device() {
    let tester = Arc::new(Recorder::ok());
    let dir = tempfile::tempdir().unwrap();
    let state = state(Arc::clone(&tester), dir.path());

    common::send(
        server::api::app(state.clone()),
        post_commit("hob-aes.zip", "slug=cooktop&host=10.0.0.1"),
    )
    .await;
    common::send(
        server::api::app(state.clone()),
        post_commit("hob-aes.zip", "slug=kitchen-hob&host=10.0.0.1"),
    )
    .await;

    assert_eq!(state.devices().len(), 1, "renamed, not duplicated");
    assert!(state.devices().contains("kitchen-hob"));
    assert!(
        !state.devices().contains("cooktop"),
        "the old slug is released"
    );

    let stored = ProfileStore::new(dir.path()).load_all();
    assert_eq!(
        stored.len(),
        1,
        "and the old pair of files is gone from disk"
    );
    assert_eq!(stored.first().unwrap().slug, "kitchen-hob");
}

/// A different appliance may not take a slug that is already in use.
///
/// Matching fingerprints update, mismatched ones conflict; overwriting would lose the first
/// appliance's profile.
#[tokio::test]
async fn a_different_appliance_cannot_take_an_occupied_slug() {
    let tester = Arc::new(Recorder::ok());
    let dir = tempfile::tempdir().unwrap();
    let state = state(Arc::clone(&tester), dir.path());

    common::send(
        server::api::app(state.clone()),
        post_commit("hob-aes.zip", "slug=appliance&host=10.0.0.1"),
    )
    .await;
    let reply = common::send(
        server::api::app(state.clone()),
        post_commit("oven-tls.zip", "slug=appliance&host=10.0.0.2"),
    )
    .await;

    assert_eq!(reply.status, StatusCode::CONFLICT);
    let stored = ProfileStore::new(dir.path()).load_all();
    assert_eq!(stored.len(), 1, "the first appliance's profile is intact");
    assert_eq!(
        stored.first().unwrap().record.manual_host.as_deref(),
        Some("10.0.0.1"),
        "and untouched"
    );
}

/// A slug that would escape the profile directory is refused at the API boundary.
#[tokio::test]
async fn a_commit_refuses_an_unusable_slug() {
    let tester = Arc::new(Recorder::ok());
    let dir = tempfile::tempdir().unwrap();
    let state = state(Arc::clone(&tester), dir.path());

    for slug in ["..%2Fevil", "a%2Fb", "", "with%20space", "a.b"] {
        let reply = common::send(
            server::api::app(state.clone()),
            post_commit("hob-aes.zip", &format!("slug={slug}&host=10.0.0.1")),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::BAD_REQUEST,
            "the slug {slug:?} reaches the filesystem as a filename and must be refused"
        );
    }
    assert!(
        tester.dialled().is_empty(),
        "and refused before anything is dialled"
    );
    assert!(state.devices().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Editing and removing a committed device.
// ---------------------------------------------------------------------------------------------

/// The manual-host retry persists the new host, so a correction survives a restart.
#[tokio::test]
async fn a_manual_host_retry_is_persisted() {
    let tester = Arc::new(Recorder::ok());
    let dir = tempfile::tempdir().unwrap();
    let state = state(Arc::clone(&tester), dir.path());

    common::send(
        server::api::app(state.clone()),
        post_commit("hob-aes.zip", "slug=cooktop&host=10.0.0.1"),
    )
    .await;

    let request = Request::builder()
        .method("POST")
        .uri("/api/onboarding/cooktop/host")
        .header("content-type", "application/json")
        .header(TOKEN_HEADER, TEST_TOKEN)
        .body(Body::from(r#"{"host":"10.0.0.99"}"#))
        .unwrap();
    let reply = common::send(server::api::app(state), request).await;
    assert_eq!(reply.status, StatusCode::OK);

    assert_eq!(
        ProfileStore::new(dir.path())
            .load_all()
            .first()
            .unwrap()
            .record
            .manual_host
            .as_deref(),
        Some("10.0.0.99"),
        "the corrected host must reach disk, or the correction dies with the process"
    );
}

/// `DELETE /api/devices/{slug}` removes the device from the registry and from disk.
///
/// A failed connection test persists the device, so a wrong address must be removable.
#[tokio::test]
async fn deleting_a_device_removes_it_from_disk_too() {
    let tester = Arc::new(Recorder::ok());
    let dir = tempfile::tempdir().unwrap();
    let state = state(Arc::clone(&tester), dir.path());

    common::send(
        server::api::app(state.clone()),
        post_commit("hob-aes.zip", "slug=cooktop&host=10.0.0.1"),
    )
    .await;

    let request = Request::builder()
        .method("DELETE")
        .uri("/api/devices/cooktop")
        .header(TOKEN_HEADER, TEST_TOKEN)
        .body(Body::empty())
        .unwrap();
    let reply = common::send(server::api::app(state.clone()), request).await;
    assert_eq!(reply.status, StatusCode::OK);

    assert!(state.devices().is_empty(), "gone from the registry");
    assert!(
        ProfileStore::new(dir.path()).load_all().is_empty(),
        "and gone from disk — otherwise it returns on the next restart"
    );
}

/// A device with a running task cannot be removed or renamed, and says why.
///
/// `spawn_device_task` detaches the task, so there is no way to stop a device's connection from here.
/// Removing the entry anyway would leave the task holding a session, and re-adding the device would
/// open a second concurrent session to the same appliance.
///
/// A shadow entry means "this device has a task", the same question `launch_task`'s duplicate guard
/// asks. The message is asserted, not just the status, because this 409 says "restart Kessel" while
/// `SlugTaken` says "choose another name".
#[tokio::test]
async fn a_connected_device_cannot_be_removed_or_renamed() {
    let tester = Arc::new(Recorder::ok());
    let dir = tempfile::tempdir().unwrap();
    let state = state(Arc::clone(&tester), dir.path());

    common::send(
        server::api::app(state.clone()),
        post_commit("hob-aes.zip", "slug=cooktop&host=10.0.0.1"),
    )
    .await;

    // Stand in for a running device task, the same way production marks one.
    state.shadows().install(
        "cooktop",
        Arc::new(parking_lot::Mutex::new(hc_core::shadow::Shadow::new())),
    );

    let request = Request::builder()
        .method("DELETE")
        .uri("/api/devices/cooktop")
        .header(TOKEN_HEADER, TEST_TOKEN)
        .body(Body::empty())
        .unwrap();
    let refused = common::send(server::api::app(state.clone()), request).await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert!(
        refused.body.contains("Restart Kessel"),
        "the refusal must name the action that resolves it; got: {}",
        refused.body
    );
    assert!(
        !refused.body.contains("choose a different name"),
        "and must NOT carry `SlugTaken`'s instruction, which is the wrong action here: {}",
        refused.body
    );

    // The rename path refuses for the same reason, through the same variant.
    let renamed = common::send(
        server::api::app(state.clone()),
        post_commit("hob-aes.zip", "slug=kitchen-hob&host=10.0.0.1"),
    )
    .await;
    assert_eq!(renamed.status, StatusCode::CONFLICT);
    assert!(
        renamed.body.contains("Restart Kessel"),
        "got: {}",
        renamed.body
    );

    // Nothing was lost by either refusal: the device is still registered and still on disk.
    assert!(state.devices().contains("cooktop"));
    assert_eq!(ProfileStore::new(dir.path()).load_all().len(), 1);
}

/// A slug conflict between two *different* appliances gets the other 409, with the other instruction.
#[tokio::test]
async fn an_occupied_slug_says_to_choose_another_name() {
    let tester = Arc::new(Recorder::ok());
    let dir = tempfile::tempdir().unwrap();
    let state = state(Arc::clone(&tester), dir.path());

    common::send(
        server::api::app(state.clone()),
        post_commit("hob-aes.zip", "slug=appliance&host=10.0.0.1"),
    )
    .await;
    let reply = common::send(
        server::api::app(state),
        post_commit("oven-tls.zip", "slug=appliance&host=10.0.0.2"),
    )
    .await;

    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert!(
        reply.body.contains("choose a different name"),
        "got: {}",
        reply.body
    );
    assert!(
        !reply.body.contains("Restart Kessel"),
        "restarting would not help here: {}",
        reply.body
    );
}

/// Deleting a device that was never onboarded is a 404, not a silent success.
#[tokio::test]
async fn deleting_an_unknown_device_is_a_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(Arc::new(Recorder::ok()), dir.path());
    let request = Request::builder()
        .method("DELETE")
        .uri("/api/devices/nonexistent")
        .header(TOKEN_HEADER, TEST_TOKEN)
        .body(Body::empty())
        .unwrap();

    let reply = common::send(server::api::app(state), request).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------------------------
// The restart.
// ---------------------------------------------------------------------------------------------

/// A committed device is still there after a restart.
///
/// The restart is modelled as a brand-new `AppState` with the store pointed at the same directory,
/// then the same load call `main` makes before `spawn_device_tasks`.
#[tokio::test]
async fn a_committed_device_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();

    {
        let first = state(Arc::new(Recorder::ok()), dir.path());
        common::send(
            server::api::app(first.clone()),
            post_commit("hob-aes.zip", "slug=cooktop&host=10.0.0.42&port=8080"),
        )
        .await;
        common::send(
            server::api::app(first.clone()),
            post_commit("oven-tls.zip", "slug=oven&host=10.0.0.43"),
        )
        .await;
        assert_eq!(first.devices().len(), 2);
    }

    // A new process: an empty registry, the same directory on disk.
    let restarted = state(Arc::new(Recorder::ok()), dir.path());
    assert!(
        restarted.devices().is_empty(),
        "a fresh AppState starts empty — this is what used to be the end of the story"
    );

    let loaded = server::onboarding::load_stored_devices(&restarted);
    assert_eq!(loaded, 2, "both devices are restored");

    let summaries = restarted.devices().summaries();
    let slugs: Vec<&str> = summaries.iter().map(|s| s.slug.as_str()).collect();
    assert_eq!(slugs, ["cooktop", "oven"]);

    let cooktop = restarted.devices().entry("cooktop").unwrap();
    assert_eq!(
        cooktop.profile.host(),
        "10.0.0.42",
        "the operator's host is restored, so the device task dials the right appliance"
    );
    assert_eq!(
        cooktop.port,
        Some(8080),
        "and the port, so a reconnect uses it too"
    );
    assert_eq!(
        cooktop.role.label(),
        "cooktop",
        "the role is recomputed from the re-parsed profile"
    );
    assert!(
        !cooktop.model.entities().is_empty(),
        "and so is the entity model — a restored device is fully functional, not a stub"
    );
}

/// A restored device is registered `Offline`, not as whatever it was when the process died.
///
/// A restored device has no socket yet (the device task dials after this), so it cannot be
/// `Connected`.
#[tokio::test]
async fn a_restored_device_starts_offline() {
    let dir = tempfile::tempdir().unwrap();
    {
        let first = state(Arc::new(Recorder::ok()), dir.path());
        common::send(
            server::api::app(first.clone()),
            post_commit("hob-aes.zip", "slug=cooktop&host=10.0.0.42"),
        )
        .await;
        assert_eq!(
            first
                .devices()
                .summaries()
                .first()
                .unwrap()
                .connection_state,
            server::types::DeviceConnectionState::Connected,
            "it was connected before the restart"
        );
    }

    let restarted = state(Arc::new(Recorder::ok()), dir.path());
    server::onboarding::load_stored_devices(&restarted);
    assert_eq!(
        restarted
            .devices()
            .summaries()
            .first()
            .unwrap()
            .connection_state,
        server::types::DeviceConnectionState::Offline,
        "a restored device has no socket until its task dials one"
    );
}

/// Loading is idempotent: a second call does not duplicate what is already registered.
#[tokio::test]
async fn loading_twice_does_not_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let state = state(Arc::new(Recorder::ok()), dir.path());
    common::send(
        server::api::app(state.clone()),
        post_commit("hob-aes.zip", "slug=cooktop&host=10.0.0.42"),
    )
    .await;

    server::onboarding::load_stored_devices(&state);
    server::onboarding::load_stored_devices(&state);
    assert_eq!(state.devices().len(), 1);
}

/// With no store installed, a commit still works — it just persists nothing.
///
/// `AppState::new` installs no store so a test that onboards a fixture writes nothing to disk;
/// production installs one in `main`.
#[tokio::test]
async fn a_commit_without_a_store_still_onboards() {
    let tester = Arc::new(Recorder::ok());
    let installed: Arc<dyn ConnectionTester> = Arc::clone(&tester) as Arc<dyn ConnectionTester>;
    let state = AppState::new(common::bus())
        .with_token(ApiToken::from_string(TEST_TOKEN))
        .with_connection_tester(installed);

    let reply = common::send(
        server::api::app(state.clone()),
        post_commit("hob-aes.zip", "slug=cooktop&host=10.0.0.42"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert!(state.devices().contains("cooktop"));
    assert_eq!(tester.dialled().len(), 1);
}
