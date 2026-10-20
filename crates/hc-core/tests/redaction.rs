//! `Debug` redaction: the shadow and the write command carry appliance values that can include
//! `deviceID`/`mac`/`serialNumber`, so neither renders them through `Debug`. Values are reached
//! through explicit accessors instead.
//!
//! Each case first shows the sentinel does appear in a raw `Value`'s `Debug`, so the check is not
//! vacuous.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "tests may panic on failed assertions"
    )
)]

use hc_core::shadow::Shadow;
use hc_core::write::WriteCommand;
use serde_json::json;

/// A stand-in for an appliance identifier that must never appear in `Debug` output.
const SENTINEL: &str = "SERIAL-DEADBEEF-MAC-00:11:22-deviceID-cafe";

#[test]
fn shadow_debug_never_renders_a_device_value() {
    // Non-vacuous: a bare Value renders the sentinel.
    let raw = json!({ "serialNumber": SENTINEL });
    assert!(
        format!("{raw:?}").contains(SENTINEL),
        "guard is non-vacuous: a raw Value does expose the sentinel"
    );

    let mut shadow = Shadow::new();
    shadow.set_optimistic(7, json!(SENTINEL));

    for rendered in [format!("{shadow:?}"), format!("{shadow:#?}")] {
        assert!(
            !rendered.contains(SENTINEL),
            "Shadow Debug leaked a device value: {rendered}"
        );
    }
    // The ShadowEntry's own Debug too.
    let entry = shadow.get(7).expect("present");
    for rendered in [format!("{entry:?}"), format!("{entry:#?}")] {
        assert!(
            !rendered.contains(SENTINEL),
            "ShadowEntry Debug leaked a device value: {rendered}"
        );
    }

    // But the value is reachable through the explicit named path.
    assert_eq!(
        entry.value().as_str(),
        Some(SENTINEL),
        "the value is available via ShadowEntry::value, just not via Debug"
    );
}

#[test]
fn write_command_debug_never_renders_its_value() {
    let command = WriteCommand {
        resource: "/ro/values".to_owned(),
        target_uid: Some(551),
        field: Some("FinishInRelative".to_owned()),
        value: json!(SENTINEL),
        validate: true,
        options: Vec::new(),
    };
    for rendered in [format!("{command:?}"), format!("{command:#?}")] {
        assert!(
            !rendered.contains(SENTINEL),
            "WriteCommand Debug leaked its value body: {rendered}"
        );
        // Routing metadata is not an appliance identifier, so it still renders.
        assert!(
            rendered.contains("/ro/values") && rendered.contains("FinishInRelative"),
            "routing metadata should still render: {rendered}"
        );
    }
    assert_eq!(
        command.value.as_str(),
        Some(SENTINEL),
        "the value is reachable via the public field, just not via Debug"
    );
}
