//! The optimistic shadow: optimistic writes reconciled by the authoritative `NOTIFY`.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use hc_core::shadow::{Origin, Shadow};
use hc_proto::ValueUpdate;
use serde_json::json;

#[test]
fn optimistic_shadow_then_notify_reconciles() {
    let mut shadow = Shadow::new();

    // A local write is reflected immediately, marked optimistic.
    shadow.set_optimistic(10, json!(5));
    let entry = shadow.get(10).expect("optimistic write is visible at once");
    assert_eq!(entry.value(), &json!(5), "the optimistic value is shown");
    assert_eq!(
        entry.origin(),
        Origin::Optimistic,
        "before the NOTIFY it is flagged optimistic"
    );

    // A DIVERGENT authoritative NOTIFY wins — the appliance is always right.
    shadow.reconcile(&ValueUpdate {
        uid: 10,
        value: Some(json!(9)),
        description: hc_proto::DescriptionUpdate::default(),
    });
    let entry = shadow.get(10).expect("still present after reconcile");
    assert_eq!(
        entry.value(),
        &json!(9),
        "a divergent NOTIFY overwrites the optimistic value"
    );
    assert_eq!(
        entry.origin(),
        Origin::Confirmed,
        "after the NOTIFY the value is confirmed, not optimistic"
    );

    // A MATCHING NOTIFY confirms the optimistic write without changing the value.
    shadow.set_optimistic(11, json!(true));
    shadow.reconcile(&ValueUpdate {
        uid: 11,
        value: Some(json!(true)),
        description: hc_proto::DescriptionUpdate::default(),
    });
    let entry = shadow.get(11).expect("present");
    assert_eq!(entry.value(), &json!(true), "the value is unchanged");
    assert_eq!(
        entry.origin(),
        Origin::Confirmed,
        "a matching NOTIFY clears the optimistic flag"
    );
}

#[test]
fn a_description_only_update_keeps_the_held_value() {
    let mut shadow = Shadow::new();
    shadow.reconcile(&ValueUpdate {
        uid: 10,
        value: Some(json!(true)),
        description: hc_proto::DescriptionUpdate::default(),
    });

    shadow.reconcile(&ValueUpdate {
        uid: 10,
        value: None,
        description: hc_proto::DescriptionUpdate {
            available: Some(false),
            ..hc_proto::DescriptionUpdate::default()
        },
    });

    let entry = shadow.get(10).expect("the value is still held");
    assert_eq!(
        entry.value(),
        &json!(true),
        "a revision without a value is not a null"
    );
}
