//! A device onboarded through the UI gets its long-lived task immediately, not after a restart.
//!
//! `spawn_device_tasks` runs once at startup, so without this a device onboarded later has no task,
//! no shadow, and no values.
//!
//! The launch path sits behind `server::onboarding::DeviceLauncher` and these tests inject a
//! recording fake. `AppState::new` defaults to the non-launching `NoDeviceTasks`, which
//! `a_test_binary_never_gets_the_live_launcher` pins. Nothing here opens a socket.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use server::onboarding::{ConnectionTarget, ConnectionTester, DeviceLauncher};
use server::state::AppState;

mod common;

/// A tester with a fixed verdict that opens no socket.
#[derive(Debug)]
struct FixedTester(Result<(), String>);

#[async_trait]
impl ConnectionTester for FixedTester {
    async fn test(&self, _target: &ConnectionTarget) -> Result<(), String> {
        self.0.clone()
    }
}

/// A launcher that records the slugs it was asked to start, and opens no socket.
///
/// It installs a shadow, because the duplicate guard reads `ShadowRegistry` as "this device has a
/// task"; without one `a_manual_host_retry_does_not_launch_a_second_task` would pass vacuously.
#[derive(Debug, Default)]
struct RecordingLauncher {
    launched: Mutex<Vec<String>>,
}

impl RecordingLauncher {
    fn launched(&self) -> Vec<String> {
        self.launched.lock().clone()
    }
}

impl DeviceLauncher for RecordingLauncher {
    fn launch(&self, state: &AppState, slug: &str) -> bool {
        self.launched.lock().push(slug.to_owned());
        state
            .shadows()
            .install(slug, Arc::new(Mutex::new(hc_core::shadow::Shadow::new())));
        true
    }
}

/// A state whose connection test yields `verdict`, with `launcher` installed.
fn state(verdict: Result<(), String>, launcher: Arc<RecordingLauncher>) -> AppState {
    AppState::new(common::bus())
        .with_connection_tester(Arc::new(FixedTester(verdict)))
        .with_device_launcher(launcher)
}

/// The operator's confirmed choices for the dishwasher fixture.
///
/// A slug and no host, so the derived host is used; these tests are about task launching, not
/// addressing.
fn dishwasher() -> server::types::CommitRequest {
    server::types::CommitRequest {
        slug: "dishwasher".to_owned(),
        host: None,
        port: None,
    }
}

/// The fix: a successful onboarding starts the device's task immediately.
#[tokio::test]
async fn onboarding_launches_the_device_task() {
    let launcher = Arc::new(RecordingLauncher::default());
    let state = state(Ok(()), Arc::clone(&launcher));

    let result = server::onboarding::commit(
        &state,
        &common::fixture_zip_bytes("dishwasher-tls.zip"),
        &dishwasher(),
    )
    .await
    .expect("the scrubbed dishwasher fixture onboards");

    assert_eq!(
        launcher.launched(),
        vec![result.device.slug],
        "a connected device must get its task at onboarding time, not at the next restart"
    );
}

/// The manual-IP escalation reaches the connection test a second time for the same device. It must not
/// produce a second task: there must never be two concurrent sessions against one appliance.
#[tokio::test]
async fn a_manual_host_retry_does_not_launch_a_second_task() {
    let launcher = Arc::new(RecordingLauncher::default());
    let state = state(Ok(()), Arc::clone(&launcher));

    let result = server::onboarding::commit(
        &state,
        &common::fixture_zip_bytes("dishwasher-tls.zip"),
        &dishwasher(),
    )
    .await
    .expect("the scrubbed dishwasher fixture onboards");
    let slug = result.device.slug.clone();

    // RFC 5737 TEST-NET-1, and nothing dials it: `FixedTester` returns its verdict without a socket.
    server::onboarding::retry_with_manual_host(&state, &slug, "192.0.2.1".to_owned())
        .await
        .expect("the device is registered");

    assert_eq!(
        launcher.launched(),
        vec![slug],
        "a device that already has a task must not be given a second one"
    );
}

/// A device that could not be reached gets no task. The supervisor would retry forever against an
/// appliance the operator has just been told to supply an address for.
#[tokio::test]
async fn a_failed_connection_test_launches_nothing() {
    let launcher = Arc::new(RecordingLauncher::default());
    let state = state(
        Err("no route to host (fake)".to_owned()),
        Arc::clone(&launcher),
    );

    server::onboarding::commit(
        &state,
        &common::fixture_zip_bytes("dishwasher-tls.zip"),
        &dishwasher(),
    )
    .await
    .expect("the fixture parses even when the connection test fails");

    assert!(
        launcher.launched().is_empty(),
        "an unreachable device must not get a task, got: {:?}",
        launcher.launched()
    );
}

/// What keeps `LiveDeviceLauncher` out of every test in this crate is which implementation
/// `AppState::new` installs, so that is asserted directly.
#[test]
fn a_test_binary_never_gets_the_live_launcher() {
    let state = AppState::new(common::bus());
    let rendered = format!("{:?}", state.device_launcher());

    assert!(
        rendered.starts_with("NoDeviceTasks"),
        "AppState::new must install NoDeviceTasks, got: {rendered}"
    );
    assert!(
        !rendered.contains("LiveDeviceLauncher"),
        "the live launcher must never be the default in a test binary, or a test could dial a real appliance, got: {rendered}"
    );
    assert!(
        state.shadows().is_empty(),
        "the default launcher must start nothing, so no shadow may exist"
    );
}
