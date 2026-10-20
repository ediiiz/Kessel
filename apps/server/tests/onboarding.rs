//! `zip_upload_parses_and_connection_tests` and `connect_failure_escalates_to_manual_ip`: a profile
//! ZIP is parsed, connection-tested, and on connect failure the flow escalates to a manual-IP prompt.
//!
//! No appliance is contacted. The connection test sits behind `server::onboarding::ConnectionTester`
//! and these tests inject a fake that records what it was asked to dial. The real
//! `LiveConnectionTester` exists in this crate, so `existing_onboarding_tests_still_use_the_fake`
//! pins that `AppState::new` still installs the refusing `NoLiveTransport`.
//!
//! The profile is the scrubbed hob fixture, with synthetic identifiers and a synthetic psk/iv.
//!
//! What the tests pin:
//!
//! * a posted ZIP is parsed (the feature model is populated) and a connection test is attempted,
//!   asserted by the fake recording the exact host it was asked to dial;
//! * the device is addressed by a slug derived from its role class, never by its `haId`, serial or
//!   MAC;
//! * a failed auto-connect answers with `needs_manual_ip`, and the manual-IP retry dials the host the
//!   operator supplied and reports `host_source: manual`;
//! * the default tester is still the refusing fake, so no test in this crate can silently start
//!   contacting hardware.
//!
//! These tests drive `commit` without a `host`, so they pin the derived host; the preview, the
//! operator-supplied host and port, the upsert and the restart are in `tests/onboarding_commit.rs`.

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

/// The token these tests authorise with — onboarding is a mutating route.
const TEST_TOKEN: &str = "test-token-not-a-real-one";

/// A connection tester that records every host it is asked to dial and never opens a socket.
#[derive(Debug, Default)]
struct RecordingTester {
    dialled: Mutex<Vec<String>>,
    /// Hosts that "succeed". Everything else fails, which is how the escalation is driven.
    reachable: Vec<String>,
}

impl RecordingTester {
    fn reachable(hosts: &[&str]) -> Self {
        Self {
            dialled: Mutex::new(Vec::new()),
            reachable: hosts.iter().map(|host| (*host).to_owned()).collect(),
        }
    }

    fn dialled(&self) -> Vec<String> {
        self.dialled.lock().clone()
    }
}

#[async_trait]
impl ConnectionTester for RecordingTester {
    async fn test(&self, target: &ConnectionTarget) -> Result<(), String> {
        self.dialled.lock().push(target.host.clone());
        if self.reachable.contains(&target.host) {
            Ok(())
        } else {
            Err("no route to host (fake)".to_owned())
        }
    }
}

/// A state with a known token and the given tester.
fn state(tester: Arc<RecordingTester>) -> AppState {
    AppState::new(common::bus())
        .with_token(ApiToken::from_string(TEST_TOKEN))
        .with_connection_tester(tester)
}

/// `POST` the manual host the operator typed.
fn post_manual_host(slug: &str, host: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/api/onboarding/{slug}/host"))
        .header("content-type", "application/json")
        .header(TOKEN_HEADER, TEST_TOKEN)
        .body(Body::from(format!(r#"{{"host":"{host}"}}"#)))
        .expect("a well-formed request")
}

/// `POST` the profile ZIP to the **commit** route, as the wizard's confirmation dialog does.
///
/// The ZIP is the body and the operator's confirmed choices are the query string. No `host` is
/// supplied, so the derived host is used.
fn post_zip() -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/onboarding/commit?slug=cooktop")
        .header("content-type", "application/zip")
        .header(TOKEN_HEADER, TEST_TOKEN)
        .body(Body::from(common::hob_zip_bytes()))
        .expect("a well-formed request")
}

#[tokio::test]
async fn zip_upload_parses_and_connection_tests() {
    // The hob profile's derived host is its `haId` (AES path). The test reads it back from the
    // recording rather than hard-coding an identifier.
    let tester = Arc::new(RecordingTester::reachable(&[]));
    let state = state(Arc::clone(&tester));

    let reply = common::send(server::api::app(state.clone()), post_zip()).await;

    // 1. It parsed: a device is registered, with the hob's several hundred features.
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "a posted profile ZIP is accepted: {}",
        reply.body
    );
    let result = reply.json();
    assert_eq!(
        result["device"]["role"], "cooktop",
        "the profile's `type: Hob` resolves to the cooktop role: {result}"
    );
    assert_eq!(
        result["device"]["slug"], "cooktop",
        "the device is addressed by a role-derived, non-identifier slug: {result}"
    );
    assert_eq!(
        result["device"]["ever_writable"], true,
        "a cooktop is writable like every other appliance, and the SPA reads this field to \
         decide whether to render a write control at all: {result}"
    );
    assert!(
        result["device"]["entity_count"].as_u64().unwrap_or(0) > 20,
        "the feature model is populated: {result}"
    );
    assert_eq!(
        state.devices().len(),
        1,
        "the parsed device is persisted in the registry"
    );

    // 2. A connection test was actually attempted: one dial, at the derived host.
    let dialled = tester.dialled();
    assert_eq!(
        dialled.len(),
        1,
        "exactly one connection test was attempted, so 'attempted' is not vacuous"
    );
    assert!(
        !dialled[0].is_empty(),
        "the connection test was given a host to dial"
    );

    // 3. And this fake failed, so the verdict is a failure with the escalation flag — the other
    // test drives the success path.
    assert_eq!(
        result["connection"]["status"], "failed",
        "an unreachable host reports failure: {result}"
    );
    assert_eq!(
        result["host_source"], "derived",
        "the first attempt uses the host derived from the manifest: {result}"
    );
}

#[tokio::test]
async fn connect_failure_escalates_to_manual_ip() {
    const MANUAL_HOST: &str = "192.0.2.11"; // TEST-NET-1 (RFC 5737): never a real appliance.

    let tester = Arc::new(RecordingTester::reachable(&[MANUAL_HOST]));
    let state = state(Arc::clone(&tester));

    // 1. The automatic attempt fails and says so in the form the wizard acts on.
    let first = common::send(server::api::app(state.clone()), post_zip()).await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    let result = first.json();
    assert_eq!(
        result["connection"]["status"], "failed",
        "the derived host is unreachable for this fake: {result}"
    );
    assert_eq!(
        result["connection"]["needs_manual_ip"], true,
        "a failed auto-connect must escalate to the manual-IP prompt: {result}"
    );
    assert!(
        result["connection"]["reason"].is_string(),
        "the failure carries an operator-facing reason: {result}"
    );

    // 2. The operator types an IP; the retry dials exactly that host and succeeds.
    let retry = common::send(
        server::api::app(state.clone()),
        post_manual_host("cooktop", MANUAL_HOST),
    )
    .await;
    assert_eq!(retry.status, StatusCode::OK, "{}", retry.body);
    let retried = retry.json();
    assert_eq!(
        retried["connection"]["status"], "connected",
        "the manual host is reachable for this fake: {retried}"
    );
    assert_eq!(
        retried["host_source"], "manual",
        "the result records that the host came from the operator: {retried}"
    );
    assert_eq!(
        tester.dialled().last().map(String::as_str),
        Some(MANUAL_HOST),
        "the retry dialled the host the operator supplied, not the derived one: {:?}",
        tester.dialled()
    );

    // 3. A manual host for a device that was never onboarded is a 404, not a silent success.
    let unknown = common::send(
        server::api::app(state),
        post_manual_host("dishwasher", MANUAL_HOST),
    )
    .await;
    assert_eq!(
        unknown.status,
        StatusCode::NOT_FOUND,
        "a manual host for an unknown slug is 404: {}",
        unknown.body
    );
}

/// Keeps hardware out of this test suite.
///
/// The only thing standing between `LiveConnectionTester` and every test in this crate is which
/// implementation `AppState::new` installs. This asserts it three ways, so a rename cannot slip
/// through:
///
/// 1. the default tester's `Debug` is `NoLiveTransport` (and is **not** `LiveConnectionTester`);
/// 2. it refuses, locally, without dialling — asserted against an RFC 5737 TEST-NET-1 host inside a
///    timeout far shorter than any TCP connect budget, so a dial would fail the test;
/// 3. the default HTTP wiring reports a *failed* connection test for a posted profile, which is the
///    end-to-end form of the same fact.
#[tokio::test]
async fn existing_onboarding_tests_still_use_the_fake() {
    use std::time::Duration;

    /// RFC 5737 TEST-NET-1. Documentation-only; never a real appliance.
    const UNROUTABLE_HOST: &str = "192.0.2.1";
    /// Long enough that a local decision is never mistaken for a timeout; far below a connect budget.
    const NO_DIAL_BUDGET: Duration = Duration::from_secs(2);

    let state = AppState::new(common::bus());

    // 1. The default really is the refusing fake, by name.
    let rendered = format!("{:?}", state.connection_tester());
    assert!(
        rendered.starts_with("NoLiveTransport"),
        "AppState::new must install NoLiveTransport, got: {rendered}"
    );
    assert!(
        !rendered.contains("LiveConnectionTester"),
        "the live tester must never be the default in a test binary, or a test could dial a real appliance, got: {rendered}"
    );

    // 2. And it refuses locally, contacting nothing.
    let target = server::onboarding::target_for(
        &hc_profile::Profile::from_zip_bytes(&common::hob_zip_bytes())
            .expect("the scrubbed hob fixture parses")
            .with_manual_host(UNROUTABLE_HOST),
        None,
    );
    let verdict = tokio::time::timeout(NO_DIAL_BUDGET, state.connection_tester().test(&target))
        .await
        .expect("the refusal is decided locally, without dialling anything");
    let reason = verdict.expect_err("the default tester must refuse, not report success");
    assert!(
        reason.contains("no connection was attempted"),
        "the refusal must say that nothing was dialled, got: {reason}"
    );

    // 3. End to end: the default HTTP wiring reports a failed connection test, never a connected one.
    let reply = common::send(
        server::api::app(
            AppState::new(common::bus()).with_token(ApiToken::from_string(TEST_TOKEN)),
        ),
        post_zip(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert_eq!(
        reply.json()["connection"]["status"],
        "failed",
        "with the default tester a posted profile must never report `connected`: {}",
        reply.body
    );
}
