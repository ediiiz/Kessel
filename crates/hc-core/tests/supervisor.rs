//! Device-supervisor behaviour, against the fake transport.

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

use std::sync::Arc;
use std::time::Duration;

use common::{Attempt, FakeLink, RunBehavior, update};
use hc_core::backoff::Backoff;
use hc_core::device::DeviceRole;
use hc_core::shadow::{Origin, Shadow};
use hc_core::supervisor::{DeviceConfig, LinkError, Supervisor};
use hc_core::trace::{TraceBus, TraceLevel};
use parking_lot::Mutex;
use serde_json::json;

const fn instant_backoff() -> Backoff {
    // Zero base and cap → zero delays → the reconnect loop runs at full speed in tests.
    Backoff::new(Duration::ZERO, Duration::ZERO, 1.0)
}

const fn config(role: DeviceRole, max_attempts: u32) -> DeviceConfig {
    DeviceConfig {
        role,
        backoff: instant_backoff(),
        rng_seed: 1,
        max_attempts: Some(max_attempts),
        shutdown: None,
    }
}

#[tokio::test]
async fn task_per_device_isolates_failures() {
    let bus = Arc::new(TraceBus::new(4096));
    let shadow = Arc::new(Mutex::new(Shadow::new()));

    let mut supervisor = Supervisor::new();

    // A healthy device: connects, closes cleanly, reconnects — three times.
    supervisor.spawn(
        FakeLink::healthy(Vec::new()),
        config(DeviceRole::Dishwasher, 3),
        Arc::clone(&bus),
        Arc::clone(&shadow),
    );
    // An always-offline device: three failed attempts.
    supervisor.spawn(
        FakeLink::always_offline(),
        config(DeviceRole::Fridge, 3),
        Arc::clone(&bus),
        Arc::clone(&shadow),
    );
    // A device whose session PANICS: its task must die alone.
    supervisor.spawn(
        FakeLink::new(vec![Attempt::Connect {
            initial: Vec::new(),
            behavior: RunBehavior::Panic,
        }]),
        config(DeviceRole::Cooktop, 3),
        Arc::clone(&bus),
        Arc::clone(&shadow),
    );

    let mut handles = supervisor.into_handles();
    let panicker = handles.pop().expect("cooktop handle");
    let offliner = handles.pop().expect("fridge handle");
    let healthy = handles.pop().expect("dishwasher handle");

    // The panicking device's task fails to join — and that is the ONLY casualty.
    let panic_join = panicker.await;
    assert!(
        panic_join.is_err(),
        "the panicking device's task must surface a JoinError, not silently succeed"
    );

    // The siblings ran to completion regardless of the panic next door.
    let healthy_outcome = healthy.await.expect("healthy sibling survives the panic");
    assert_eq!(
        healthy_outcome.connects, 3,
        "the healthy device kept connecting while a sibling panicked"
    );
    let offline_outcome = offliner.await.expect("offline sibling survives the panic");
    assert_eq!(
        offline_outcome.failures, 3,
        "the offline device kept retrying while a sibling panicked"
    );
}

#[tokio::test]
async fn offline_is_not_an_error_log_flood() {
    const RETRIES: u32 = 500;

    let bus = Arc::new(TraceBus::new(4096));
    let shadow = Arc::new(Mutex::new(Shadow::new()));

    // A device offline for the whole run, retried RETRIES times.
    hc_core::supervisor::run_device(
        FakeLink::always_offline(),
        config(DeviceRole::Dishwasher, RETRIES),
        Arc::clone(&bus),
        Arc::clone(&shadow),
    )
    .await;

    let errors = bus
        .recent()
        .into_iter()
        .filter(|event| event.level == TraceLevel::Error)
        .count();

    assert!(
        errors <= 1,
        "a persistently-offline device must not emit an error per retry: {RETRIES} retries \
         produced {errors} error events (offline is normal for an appliance)"
    );
    assert!(
        bus.emitted() >= u64::from(RETRIES),
        "the device did keep retrying — the low error count is quiet retries, not a stalled loop"
    );
}

#[tokio::test]
async fn recovered_device_logs_a_fresh_error_on_the_next_outage() {
    // One error per outage, not one error ever: fail, recover, fail again gives exactly two
    // error-level events.
    let bus = Arc::new(TraceBus::new(4096));
    let shadow = Arc::new(Mutex::new(Shadow::new()));

    let script = vec![
        Attempt::Fail(LinkError::Offline("outage 1".to_owned())),
        Attempt::Connect {
            initial: Vec::new(),
            behavior: RunBehavior::CloseCleanly,
        },
        Attempt::Fail(LinkError::Offline("outage 2".to_owned())),
    ];

    hc_core::supervisor::run_device(
        FakeLink::new(script),
        config(DeviceRole::Fridge, 3),
        Arc::clone(&bus),
        Arc::clone(&shadow),
    )
    .await;

    let errors = bus
        .recent()
        .into_iter()
        .filter(|event| event.level == TraceLevel::Error)
        .count();
    assert_eq!(
        errors, 2,
        "two distinct outages must produce two error events — one per outage, not deduped forever"
    );
}

#[tokio::test]
async fn initial_ro_state_seeds_each_device_shadow() {
    // The decoded post-CONNECT /ro state seeds the device shadow on connect.
    let bus = Arc::new(TraceBus::new(4096));
    let shadow = Arc::new(Mutex::new(Shadow::new()));

    let initial = vec![update(20, json!(1)), update(21, json!("Run"))];

    hc_core::supervisor::run_device(
        FakeLink::healthy(initial),
        config(DeviceRole::Dishwasher, 1),
        Arc::clone(&bus),
        Arc::clone(&shadow),
    )
    .await;

    let guard = shadow.lock();
    let len = guard.len();
    let twenty_value = guard.get(20).map(|e| (e.value().clone(), e.origin()));
    let twenty_one_value = guard.get(21).map(|e| e.value().clone());
    drop(guard);

    assert_eq!(len, 2, "both /ro entries seeded the shadow");
    assert_eq!(
        twenty_value,
        Some((json!(1), Origin::Confirmed)),
        "uid 20 seeded with its value, marked authoritative not optimistic"
    );
    assert_eq!(
        twenty_one_value,
        Some(json!("Run")),
        "the second /ro entry seeded too"
    );
}

/// A `NOTIFY` arriving mid-connection reconciles the shadow, not only the seed taken at connect.
/// The scripted session seeds uid 20 with `1`, then delivers `2` for the same uid.
#[tokio::test]
async fn a_notify_reconciles_the_shadow_during_steady_state() {
    let bus = Arc::new(TraceBus::new(4096));
    let shadow = Arc::new(Mutex::new(Shadow::new()));

    hc_core::supervisor::run_device(
        FakeLink::new(vec![Attempt::Connect {
            initial: vec![update(20, json!(1))],
            behavior: RunBehavior::NotifyThenClose(vec![
                update(20, json!(2)),
                update(21, json!(7)),
            ]),
        }]),
        config(DeviceRole::Dishwasher, 1),
        Arc::clone(&bus),
        Arc::clone(&shadow),
    )
    .await;

    let guard = shadow.lock();
    let twenty = guard.get(20).map(|e| (e.value().clone(), e.origin()));
    let twenty_one = guard.get(21).map(|e| e.value().clone());
    let len = guard.len();
    drop(guard);

    assert_eq!(
        twenty,
        Some((json!(2), Origin::Confirmed)),
        "the NOTIFY's value must replace the seeded one, and stay authoritative"
    );
    assert_eq!(
        twenty_one,
        Some(json!(7)),
        "a uid first seen in steady state must be added, not dropped"
    );
    assert_eq!(len, 2, "one entry per uid — reconciling is not appending");
}

/// A session that ends is closed, not merely dropped: three connect-and-close cycles produce three
/// closes.
///
/// The appliance tracks clients in a bounded table indexed by `endDeviceID`, and learns a session
/// is over from a WebSocket Close frame; an abandoned session occupies a slot.
#[tokio::test]
async fn a_finished_session_is_closed_not_just_dropped() {
    let bus = Arc::new(TraceBus::new(4096));
    let shadow = Arc::new(Mutex::new(Shadow::new()));

    let link = FakeLink::healthy(Vec::new());
    let closes = Arc::clone(&link.closes);

    let mut supervisor = Supervisor::new();
    supervisor.spawn(
        link,
        config(DeviceRole::Cooktop, 3),
        Arc::clone(&bus),
        Arc::clone(&shadow),
    );
    for handle in supervisor.into_handles() {
        handle.await.expect("the device task runs to completion");
    }

    assert_eq!(
        *closes.lock(),
        3,
        "each of the three sessions must be closed on the way out, so the appliance is told \
         rather than left to infer it from a dropped socket"
    );
}

/// Shutdown interrupts a live session, closes it, and ends the task.
///
/// `run_steady_state` does not return while a connection is healthy, so without this a daemon that
/// exits drops the socket and the appliance is never told.
#[tokio::test(start_paused = true)]
async fn shutdown_closes_a_live_session_and_stops_the_task() {
    let bus = Arc::new(TraceBus::new(4096));
    let shadow = Arc::new(Mutex::new(Shadow::new()));

    // A session that never ends on its own — what a healthy appliance looks like.
    let link = FakeLink::new(vec![Attempt::Connect {
        initial: Vec::new(),
        behavior: RunBehavior::Hang,
    }]);
    let closes = Arc::clone(&link.closes);

    let (signal, receiver) = tokio::sync::watch::channel(false);
    let mut config = config(DeviceRole::Cooktop, 1);
    config.shutdown = Some(receiver);

    let mut supervisor = Supervisor::new();
    supervisor.spawn(link, config, Arc::clone(&bus), Arc::clone(&shadow));

    // Let the task reach steady state before asking it to stop.
    tokio::task::yield_now().await;
    signal
        .send(true)
        .expect("the device task holds the receiver");

    for handle in supervisor.into_handles() {
        let finished = tokio::time::timeout(Duration::from_secs(5), handle).await;
        assert!(
            finished.is_ok(),
            "a device task must stop when told to shut down, rather than hanging on a session \
             that will never end by itself"
        );
        finished
            .unwrap()
            .expect("the device task runs to completion");
    }

    assert_eq!(
        *closes.lock(),
        1,
        "the live session must be closed on the way out, so the appliance is told before the \
         process goes away"
    );
}
