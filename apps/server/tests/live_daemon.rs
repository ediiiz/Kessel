//! `live_daemon_connection_test_reaches_connected`: the live onboarding connection test.
//!
//! # Device safety
//!
//! This test contacts real appliances, including a cooktop and an oven. It is read-only: it drives
//! the daemon's onboarding connection test (`connect` + `run_handshake()`, then drops the session).
//! No command, setting, option or write is sent. That is a property of this test, not of the
//! daemon, which would accept a write to any appliance.
//!
//! Two gates must both be open:
//!
//! 1. `#[ignore]`: only `just live` (which passes `--ignored`) runs it;
//! 2. `KESSEL_LIVE_PROFILE_DIR` pointing at a directory of profile ZIPs outside the repo.
//!
//! Every skip prints why.
//!
//! These appliances accept a second concurrent session and then both return garbage, so this is one
//! test that walks the appliances sequentially, closing each session before the next dial.
//!
//! Per appliance, it asserts that the connection test reaches `ConnectionTest::Connected`, the
//! handshake outcome is `State::Connected`, and no 403 was seen. A powered-off appliance is reported
//! with its exact error and the walk continues.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]
#![allow(
    clippy::print_stderr,
    reason = "a live run is read off the terminal: the result table and every skip reason must \
              be visible under `--nocapture`, and no `tracing` subscriber is installed"
)]

mod common;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hc_core::device::{ConnectionState, DeviceRole};
use hc_profile::Profile;
use hc_proto::State;
use server::live_transport::{LiveConnectionTester, select_transport};
use server::onboarding::{ConnectionTester, role_for_type, target_for};
use server::state::{AppState, DeviceEntry};

/// The directory holding the appliance profile ZIPs. Never a path inside the repo.
const PROFILE_DIR_ENV: &str = "KESSEL_LIVE_PROFILE_DIR";

/// How long the appliance is left alone between two connections, so the previous session is fully
/// torn down before the next dial: never two concurrent sessions.
const SETTLE: Duration = Duration::from_secs(1);

/// The address to dial for `role`, from `KESSEL_LIVE_HOST_<ROLE>` (e.g. `KESSEL_LIVE_HOST_OVEN`).
/// There is no default: a role without one is skipped.
fn host_for(role: &DeviceRole) -> Option<String> {
    std::env::var(format!(
        "KESSEL_LIVE_HOST_{}",
        role.label().to_ascii_uppercase()
    ))
    .ok()
}

/// One appliance's result. Carries no identifier: the role class, the transport, the verdicts,
/// `saw_403`, the TLS identity and a duration.
struct Row {
    role: String,
    transport: String,
    /// The verdict of the daemon's connection test at the derived host.
    derived_host_verdict: String,
    /// The verdict at the known LAN address, verbatim from the API's `ConnectionTest`.
    verdict: String,
    saw_403: Option<bool>,
    psk_identity: Option<String>,
    elapsed: Duration,
}

impl fmt::Display for Row {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{role:<11} {transport:<7} {elapsed:>8.2?}  403={saw_403:<8} \
             psk_identity={identity:<20} manual_host={verdict}  derived_host={derived}",
            role = self.role,
            transport = self.transport,
            elapsed = self.elapsed,
            saw_403 = self
                .saw_403
                .map_or_else(|| "n/a".to_owned(), |seen| seen.to_string()),
            identity = self
                .psk_identity
                .as_ref()
                .map_or_else(|| "n/a".to_owned(), |id| format!("{id:?}")),
            verdict = self.verdict,
            derived = self.derived_host_verdict,
        )
    }
}

/// The `.zip` profiles in `dir`, in a deterministic order so a run is reproducible.
fn profiles_in(dir: &str) -> Vec<PathBuf> {
    let mut zips: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("the profile directory is readable")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "zip"))
        .collect();
    zips.sort();
    zips
}

/// Drives the daemon's onboarding flow against one appliance and returns its row.
///
/// `None` if the profile's role has no known LAN address, which is an explicit, printed skip. Every
/// assertion that can fail is inside, so a failure names the appliance it belongs to.
async fn exercise_one(zip: &Path) -> Option<Row> {
    // Never `zip.display()`: a real profile's filename carries the serial or haId, which must not
    // be logged. Failures are attributed by role class instead.
    let bytes = std::fs::read(zip).expect("the profile ZIP is readable");
    let profile = Profile::from_zip_bytes(&bytes).expect("the profile ZIP parses");
    let role = role_for_type(&profile.identity().device_type);
    let slug = role.label().to_owned();
    let host = host_for(&role).or_else(|| {
        eprintln!("SKIP: no LAN address is known for role `{slug}`; contacting nothing.");
        None
    })?;

    // `None`: the live fleet is on the transports' default ports; the port override is opt-in.
    let target = target_for(&profile, None);
    let transport = format!("{:?}", select_transport(&target));

    let tester = Arc::new(LiveConnectionTester::new());
    let installed: Arc<dyn ConnectionTester> = Arc::<LiveConnectionTester>::clone(&tester);
    let state = AppState::new(common::bus()).with_connection_tester(installed);
    state.devices().insert(
        &slug,
        DeviceEntry::from_profile(&slug, role.clone(), &profile),
    );

    // 1. The host derived from the manifest. If it fails at name resolution, no socket was opened
    //    at all.
    let derived_host_verdict = match tester.test(&target).await {
        Ok(()) => "connected".to_owned(),
        Err(reason) => format!("failed: {reason}"),
    };
    tokio::time::sleep(SETTLE).await;

    // 2. The known LAN address, through the daemon's own manual-IP escalation: the real
    //    `onboarding::connection_test`, not a shortcut around it.
    let started = Instant::now();
    let result = server::onboarding::retry_with_manual_host(&state, &slug, host)
        .await
        .expect("the device was just registered under this slug");
    let elapsed = started.elapsed();
    let verdict = serde_json::to_string(&result.connection).expect("the verdict serialises");
    let handshake = tester.last_outcome();

    if verdict.contains("\"connected\"") {
        let handshake = handshake.expect("a connected verdict records a handshake outcome");
        assert_eq!(
            handshake.state,
            State::Connected,
            "{slug}: a `connected` verdict must come from a CONNECTED handshake"
        );
        // A 403 during the handshake is a regression, asserted for every appliance that connects.
        assert!(
            !handshake.saw_403,
            "REGRESSION: {slug} answered a handshake step with code 403"
        );
        assert_eq!(
            state.devices().entry(&slug).map(|entry| entry.state),
            Some(ConnectionState::Connected),
            "{slug}: a successful connection test must leave the registry `connected`"
        );
    }

    Some(Row {
        role: slug,
        transport,
        derived_host_verdict,
        verdict,
        saw_403: handshake.map(|outcome| outcome.saw_403),
        psk_identity: tester.last_psk_identity(),
        elapsed,
    })
}

#[tokio::test]
#[ignore = "live: contacts the real appliances. READ ONLY. Needs KESSEL_LIVE_PROFILE_DIR; run it deliberately, never in a gate sweep."]
async fn live_daemon_connection_test_reaches_connected() {
    // The profile-directory gate — the only one there is, besides `#[ignore]`.
    let Ok(dir) = std::env::var(PROFILE_DIR_ENV) else {
        eprintln!("SKIP: {PROFILE_DIR_ENV} is unset, so no appliance profile is available.");
        return;
    };
    // A profile directory inside the repo would mean real key material in the tree.
    assert!(
        !Path::new(&dir).starts_with(common::workspace_root()),
        "{PROFILE_DIR_ENV} must point outside the repository, so real key material never lands in the tree"
    );

    let zips = profiles_in(&dir);
    assert!(
        !zips.is_empty(),
        "{PROFILE_DIR_ENV} holds no .zip profile; nothing to exercise"
    );

    // SERIAL. One appliance, one connection at a time, each closed before the next is dialled.
    let mut rows = Vec::new();
    for zip in zips {
        if let Some(row) = exercise_one(&zip).await {
            rows.push(row);
        }
        tokio::time::sleep(SETTLE).await;
    }

    eprintln!("\n--- J-0a live results, per appliance ---");
    for row in &rows {
        eprintln!("{row}");
    }
    eprintln!("---\n");

    let connected = rows
        .iter()
        .filter(|row| row.verdict.contains("\"connected\""))
        .count();
    assert!(
        connected > 0,
        "no appliance reached CONNECTED through the daemon; the rows above are the evidence"
    );
}
