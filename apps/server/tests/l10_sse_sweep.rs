//! `sse_frames_emit_no_secret_values`: the SSE stream never carries an identifier or a key.
//!
//! Every needle is loaded at runtime and none is written into this file. A leak is reported by
//! needle source and surface only, never by value. The needles are:
//!
//! * the synthetic corpus identifiers plus `key.psk64`/`key.iv64`, from
//!   `fixtures/aes/hob-session.json`;
//! * the scrubbed hob profile's own haId / serial / mac / psk / iv, from the fixture the session is
//!   driven with;
//! * the real appliance identifiers from `secrets/` when `KESSEL_RAW_PROFILES` points at them, and a
//!   loud stderr skip otherwise.
//!
//! The real server is driven: the real `observability::subscriber` (which also proves the inspector
//! receives `info`/`debug`/`trace` with `KESSEL_LOG` unset), the onboarding route with a real
//! profile ZIP, `hc_core::supervisor::run_device` over a fake link, and the SSE route over HTTP. The
//! swept surface is the response bytes, so `event:`/`id:` metadata is swept too. The link and the
//! connection tester are in-process fakes; no socket is opened to any appliance.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "tests may panic on failed assertions; a missing key in the corpus JSON must \
                  panic loudly rather than silently load an empty needle set"
    )
)]

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::http::StatusCode;
use hc_core::backoff::Backoff;
use hc_core::device::DeviceRole;
use hc_core::shadow::Shadow;
use hc_core::supervisor::{DeviceConfig, DeviceLink, DeviceSession, LinkError, run_device};
use hc_profile::Profile;
use hc_proto::ValueUpdate;
use parking_lot::Mutex;
use serde_json::Value;
use server::auth::{ApiToken, TOKEN_HEADER};
use server::onboarding::{ConnectionTarget, ConnectionTester};
use server::state::AppState;

/// The token the onboarding POST authorises with.
const TEST_TOKEN: &str = "test-token-not-a-real-one";

/// How long the SSE reader waits for the frames. They are already in the ring buffer, so the real
/// path takes microseconds; this only bounds the failure mode so it fails instead of hanging.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Puts `value` events on the swept surface.
///
/// `run_device` over a fake link produces none: they come from the real `DecodingSink`, which needs
/// a live `hc_transport::Session`. They are emitted here with the production field names, so an
/// identifier added to a value event is caught.
fn emit_value_events(bus: &hc_core::trace::TraceBus) {
    for uid in [539_u16, 512] {
        bus.emit(
            hc_core::trace::TraceRecord::new(
                hc_core::trace::TraceLevel::Debug,
                "kessel::values",
                "feature value",
            )
            .with_field(server::types::VALUE_DEVICE_FIELD, "cooktop")
            .with_field(server::types::VALUE_UID_FIELD, uid.to_string())
            .with_field(server::types::VALUE_FIELD, format!("value-of-{uid}")),
        );
    }
}

/// A synthetic value injected into a **throwaway copy** of the captured surface to prove the sweep
/// is non-vacuous. Not a real identifier, so it is safe to name here.
const SENTINEL: &str = "L10-SSE-SWEEP-SENTINEL-VALUE";

// ================================================================================================
// Needles — every one loaded at runtime.
// ================================================================================================

/// A value that must not appear on the swept surface. Never rendered by the sweep.
#[derive(Debug, Clone)]
struct Needle {
    source: &'static str,
    value: String,
}

/// Records `value` if it is long enough to be a genuine identifier rather than a substring that
/// would match by accident.
fn push_needle(needles: &mut Vec<Needle>, source: &'static str, value: &str) {
    if value.len() >= 6 {
        needles.push(Needle {
            source,
            value: value.to_owned(),
        });
    }
}

/// The golden AES corpus's synthetic identifiers and its psk/iv.
fn corpus_needles(needles: &mut Vec<Needle>) {
    let path = common::workspace_root()
        .join("fixtures")
        .join("aes")
        .join("hob-session.json");
    let text = std::fs::read_to_string(&path).expect("the golden corpus is readable");
    let corpus: Value = serde_json::from_str(&text).expect("the corpus parses");
    if let Some(items) = corpus["synthetic_identifiers"].as_array() {
        for item in items {
            if let Some(value) = item["synthetic"].as_str() {
                push_needle(needles, "corpus synthetic_identifiers", value);
            }
        }
    }
    for key in ["psk64", "iv64"] {
        push_needle(
            needles,
            "corpus key",
            corpus["key"][key].as_str().unwrap_or_default(),
        );
    }
}

/// The identity and credentials of the profile the session is driven with.
fn profile_needles(needles: &mut Vec<Needle>, profile: &Profile, source: &'static str) {
    let identity = profile.identity();
    push_needle(needles, source, identity.ha_id.expose_secret());
    if let Some(serial) = identity.serial_number.as_ref() {
        push_needle(needles, source, serial.expose_secret());
    }
    if let Some(mac) = identity.mac.as_ref() {
        push_needle(needles, source, mac.expose_secret());
    }
    let credentials = profile.credentials();
    push_needle(needles, source, credentials.psk64.expose_secret());
    if let Some(iv) = credentials.iv64.as_ref() {
        push_needle(needles, source, iv.expose_secret());
    }
}

/// The real appliance identifiers from `secrets/`, when `KESSEL_RAW_PROFILES` points at them.
///
/// A loud stderr skip otherwise: `secrets/` is gitignored and absent from a fresh checkout and
/// from CI, and a skip is never silent.
fn secrets_needles(needles: &mut Vec<Needle>) {
    let Some(dir) = raw_profiles_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        announce(&format!(
            "SKIP sse_frames_emit_no_secret_values (secrets subset): {} is unreadable",
            dir.display()
        ));
        return;
    };
    let mut found = 0_usize;
    for path in entries.flatten().map(|entry| entry.path()) {
        if path.extension().is_none_or(|ext| ext != "zip") {
            continue;
        }
        if let Ok(profile) = Profile::from_zip_path(&path) {
            profile_needles(needles, &profile, "secrets/ real identity");
            found += 1;
        }
    }
    if found == 0 {
        announce(&format!(
            "SKIP sse_frames_emit_no_secret_values (secrets subset): no readable profile ZIPs in {}",
            dir.display()
        ));
    }
}

/// Resolves `KESSEL_RAW_PROFILES` the way `hc-core`'s sweep does: `1`/`true` → `<workspace>/secrets`.
fn raw_profiles_dir() -> Option<PathBuf> {
    let value = std::env::var("KESSEL_RAW_PROFILES")
        .ok()
        .unwrap_or_default();
    if value.is_empty() {
        announce(
            "SKIP sse_frames_emit_no_secret_values (secrets subset): KESSEL_RAW_PROFILES is not \
             set, so secrets/-derived needles were not swept (the corpus, the driven profile's own \
             identity, and the profile identity needles were)",
        );
        return None;
    }
    if value == "1" || value == "true" {
        Some(common::workspace_root().join("secrets"))
    } else {
        Some(PathBuf::from(value))
    }
}

/// Announces a skipped subset on the process's real stderr, bypassing libtest's capture.
fn announce(message: &str) {
    use std::io::Write as _;
    let mut stderr = std::io::stderr();
    let _ = writeln!(stderr, "{message}");
    let _ = stderr.flush();
}

/// Every needle occurrence, described by source only, never by value.
fn find_leaks(surface: &[String], needles: &[Needle]) -> Vec<String> {
    let mut leaks = Vec::new();
    for needle in needles {
        if surface.iter().any(|text| text.contains(&needle.value)) {
            leaks.push(format!(
                "a needle from `{}` leaked into the SSE surface",
                needle.source
            ));
        }
    }
    leaks
}

// ================================================================================================
// The fakes: no socket, to an appliance or otherwise.
// ================================================================================================

/// A connection tester that always fails and records nothing. Onboarding's failure path is the one
/// that renders a *reason* into a frame, which is where an identifier could leak.
#[derive(Debug)]
struct FailingTester;

#[async_trait]
impl ConnectionTester for FailingTester {
    async fn test(&self, _target: &ConnectionTarget) -> Result<(), String> {
        Err("no route to host (fake)".to_owned())
    }
}

/// A device link that connects once and ends, so the supervisor emits a full
/// connecting → connected → reconnecting cycle onto the bus.
struct FakeLink;

/// The session it produces. Its `/ro` initial state carries innocuous values: a fixture value that
/// contained an identifier would make this sweep prove the fixture, not the code.
struct FakeSession;

#[async_trait]
impl DeviceSession for FakeSession {
    fn initial_state(&self) -> Vec<ValueUpdate> {
        vec![ValueUpdate {
            uid: 539,
            value: Some(Value::from(1)),
            description: hc_proto::DescriptionUpdate::default(),
        }]
    }

    async fn run_steady_state(
        &mut self,
        _sink: &mut dyn hc_core::supervisor::UpdateSink,
    ) -> Result<(), LinkError> {
        Err(LinkError::Ended("fake session ended".to_owned()))
    }
}

#[async_trait]
impl DeviceLink for FakeLink {
    type Session = FakeSession;

    async fn connect(&mut self) -> Result<Self::Session, LinkError> {
        Ok(FakeSession)
    }
}

// ================================================================================================
// The test.
// ================================================================================================

/// The write refusal and the manual-host retry: both put decisions and reasons on the bus, which
/// is where a careless implementation would render an identifier.
async fn drive_write_and_retry(state: &AppState) {
    let write = axum::http::Request::builder()
        .method("POST")
        .uri("/api/devices/cooktop/write")
        .header("content-type", "application/json")
        .header(TOKEN_HEADER, TEST_TOKEN)
        .body(axum::body::Body::from(
            r#"{"feature_key":"BSH.Common.Setting.PowerState","value":"On"}"#,
        ))
        .expect("a well-formed request");
    let refused = common::send(server::api::app(state.clone()), write).await;
    assert_ne!(
        refused.status,
        StatusCode::OK,
        "the hob is never writable: {}",
        refused.body
    );

    let retry = axum::http::Request::builder()
        .method("POST")
        .uri("/api/onboarding/cooktop/host")
        // RFC 5737 TEST-NET-1: never a real appliance.
        .header("content-type", "application/json")
        .header(TOKEN_HEADER, TEST_TOKEN)
        .body(axum::body::Body::from(r#"{"host":"192.0.2.11"}"#))
        .expect("a well-formed request");
    let retried = common::send(server::api::app(state.clone()), retry).await;
    assert_eq!(
        retried.status,
        StatusCode::OK,
        "the manual-host retry must run: {}",
        retried.body
    );
}

async fn drive_and_capture(state: &AppState) -> String {
    // 1. The real onboarding route: a real profile ZIP through `hc-profile`, a failed connection
    // test, and the trace events both emit.
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/onboarding/commit?slug=cooktop")
        .header("content-type", "application/zip")
        .header(TOKEN_HEADER, TEST_TOKEN)
        .body(axum::body::Body::from(common::hob_zip_bytes()))
        .expect("a well-formed request");
    let reply = common::send(server::api::app(state.clone()), request).await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "the driven session must really onboard: {}",
        reply.body
    );

    drive_write_and_retry(state).await;

    // 2. The real supervisor lifecycle over a fake link: one connect → steady → reconnect cycle,
    // emitting ConnectionState transitions to the same bus.
    run_device(
        FakeLink,
        DeviceConfig {
            role: DeviceRole::Cooktop,
            backoff: Backoff::new(Duration::ZERO, Duration::ZERO, 1.0),
            rng_seed: 1,
            max_attempts: Some(1),
            shutdown: None,
        },
        Arc::clone(state.bus()),
        Arc::new(Mutex::new(Shadow::new())),
    )
    .await;

    // 2b. `value` events; see `emit_value_events` for why they are emitted here.
    emit_value_events(state.bus());

    // 3. Read the SSE stream over HTTP. The backlog replay carries everything emitted above.
    let expected = state.bus().recent().len();
    assert!(
        expected > 5,
        "the driven session must produce a substantial surface, got {expected} events"
    );
    let (status, _headers, body) = common::sse_body(
        server::api::app(state.clone()),
        "/api/events",
        expected,
        READ_TIMEOUT,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the SSE route answers 200");

    // `common::sse_body` stops at `min_records` or its deadline and returns whatever arrived, so a
    // short read would shrink the swept surface and still pass. The record count is asserted.
    let delivered = body.matches("data:").count();
    assert!(
        delivered >= expected,
        "the SSE read was short: {delivered} `data:` records arrived but the bus held {expected}. \
         The sweep below would have run against a truncated surface. Either the read deadline \
         ({READ_TIMEOUT:?}) is too tight or the backlog replay is dropping events."
    );
    body
}

#[test]
fn sse_frames_emit_no_secret_values() {
    let bus = common::bus();
    let state = AppState::new(Arc::clone(&bus))
        .with_token(ApiToken::from_string(TEST_TOKEN))
        .with_connection_tester(Arc::new(FailingTester));

    // The real subscriber, installed thread-locally: `#[tokio::test]` would move the work onto
    // worker threads the thread-local subscriber does not cover, so the runtime is built by hand
    // inside `with_default`.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime");
    let body = tracing::subscriber::with_default(server::observability::subscriber(&bus), || {
        runtime.block_on(drive_and_capture(&state))
    });

    // The surface: every line of the SSE response, so `event:` and `id:` metadata is swept too.
    let surface: Vec<String> = body.lines().map(str::to_owned).collect();

    // The `value` events really are on the swept surface, otherwise 2b's emission was dropped.
    assert!(
        surface.iter().any(|line| line.contains("feature_value")),
        "the `value` event type must appear in the swept surface, or value events go unswept"
    );
    assert!(
        surface
            .iter()
            .any(|line| line.contains("event: connection")),
        "the supervisor's ConnectionState transitions must be in the swept surface, or the sweep \
         is not looking at a real session:\n{body}"
    );
    assert!(
        surface.iter().any(|line| line.contains("parsed a profile")),
        "the onboarding trace events must be in the swept surface — this is also the proof that \
         the inspector sees `info` events with KESSEL_LOG unset:\n{body}"
    );

    let mut needles = Vec::new();
    let profile = Profile::from_zip_bytes(&common::hob_zip_bytes()).expect("the fixture parses");
    profile_needles(&mut needles, &profile, "the driven profile's own identity");
    corpus_needles(&mut needles);
    secrets_needles(&mut needles);
    assert!(
        needles.len() >= 5,
        "the needle set is too small to be meaningful: {} loaded",
        needles.len()
    );

    // Non-vacuity: the same sweep, over a throwaway copy with a sentinel planted in it, must fire.
    let mut planted = surface.clone();
    planted.push(format!("data: {{\"marker\":\"{SENTINEL}\"}}"));
    let mut with_sentinel = needles.clone();
    with_sentinel.push(Needle {
        source: "injected sentinel (non-vacuity control)",
        value: SENTINEL.to_owned(),
    });
    assert_eq!(
        find_leaks(&planted, &with_sentinel).len(),
        1,
        "the sweep failed to detect a planted sentinel, so a zero result would prove nothing"
    );

    // And the real surface is clean.
    let leaks = find_leaks(&surface, &needles);
    assert!(
        leaks.is_empty(),
        "the SSE surface leaked {} identifier or key needle(s): {leaks:?}",
        leaks.len()
    );
    assert!(
        !surface.iter().any(|line| line.contains(SENTINEL)),
        "the sentinel must exist only in the throwaway copy"
    );
}
