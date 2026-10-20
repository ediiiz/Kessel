//! A device's current values are readable outside its device task.
//!
//! An entry exists while a device task does, reconnects included, because the last value an
//! appliance reported is still its last value after a brief outage. That is unlike
//! `WriterRegistry`, whose entry lasts one connection so a write to a dead socket fails closed.
//! Reachability is reported separately by `ConnectionState`.
//!
//! The shadows are populated directly, which is the same call the supervisor's sink makes.

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

use hc_core::shadow::{Origin, Shadow};
use hc_core::trace::TraceBus;
use hc_proto::ValueUpdate;
use parking_lot::Mutex;
use serde_json::json;
use server::state::AppState;

/// One update, as `hc_proto::apply_updates` would produce it: a value, revising no description.
fn update(uid: u64, value: serde_json::Value) -> ValueUpdate {
    ValueUpdate {
        uid,
        value,
        description: hc_proto::DescriptionUpdate::default(),
    }
}

#[test]
fn a_value_is_readable_outside_the_device_task() {
    let state = AppState::new(Arc::new(TraceBus::new(64)));

    // What a device task does when it reaches CONNECTED: register its shadow, then seed it.
    let shadow = Arc::new(Mutex::new(Shadow::new()));
    state.shadows().install("dishwasher", Arc::clone(&shadow));
    shadow.lock().seed(&[update(41, json!(0))]);

    let read = state
        .shadows()
        .shadow("dishwasher")
        .expect("the device's shadow is registered");
    let entry = read.lock().get(41).map(|e| (e.value().clone(), e.origin()));
    assert_eq!(
        entry,
        Some((json!(0), Origin::Confirmed)),
        "a value seeded inside the device task is readable through the registry"
    );

    // The same `Arc`, not a copy: a later update is visible to a reader that already looked.
    shadow.lock().reconcile(&update(41, json!(1)));
    let after = read.lock().get(41).map(|e| e.value().clone());
    assert_eq!(
        after,
        Some(json!(1)),
        "the registry hands out the live shadow, so an update after the lookup is seen"
    );
}

#[test]
fn a_device_with_no_task_has_no_shadow() {
    let state = AppState::new(Arc::new(TraceBus::new(64)));
    assert!(
        state.shadows().shadow("dishwasher").is_none(),
        "no task, no entry — 'absent' must be distinguishable from 'reports nothing'"
    );

    state
        .shadows()
        .install("dishwasher", Arc::new(Mutex::new(Shadow::new())));
    assert!(state.shadows().shadow("dishwasher").is_some());

    // The task ends: the entry goes.
    state.shadows().remove("dishwasher");
    assert!(
        state.shadows().shadow("dishwasher").is_none(),
        "a device whose task is gone leaves no values to be read as current"
    );
}
