//! Appliance values on the REST surface.
//!
//! Three properties:
//!
//! 1. a feature's current value appears on its descriptor;
//! 2. an enumeration appears as its declared label, so the SPA can match it against the `options`
//!    the same descriptor carries (an ordinal matches nothing). MQTT uses the same rule and the
//!    same implementation;
//! 3. "no live session" and "no value" are different. A device whose task is not running has no
//!    shadow at all, and rendering it as a device whose features all read empty would show a
//!    disconnected appliance as a working one.
//!
//! The shadow is populated directly, which is the same call the supervisor's sink makes.

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

use hc_core::device::DeviceRole;
use hc_core::shadow::Shadow;
use hc_profile::Profile;
use hc_proto::ValueUpdate;
use parking_lot::Mutex;
use serde_json::json;
use server::state::{AppState, DeviceEntry};

mod common;

const SLUG: &str = "dishwasher";

/// A state with the scrubbed dishwasher onboarded, and no device task.
fn state() -> AppState {
    let state = AppState::new(common::bus());
    let profile = Profile::from_zip_bytes(&common::fixture_zip_bytes("dishwasher-tls.zip"))
        .expect("the fixture parses");
    state.devices().insert(
        SLUG,
        DeviceEntry::from_profile(SLUG, DeviceRole::Dishwasher, &profile),
    );
    state
}

/// The uid, declared ordinal and declared label of a readable enumeration on the dishwasher.
fn enum_feature(state: &AppState) -> (u16, i64, String) {
    let entry = state.devices().entry(SLUG).expect("registered");
    let mapped = entry
        .mapping
        .entries()
        .iter()
        .find(|mapped| {
            mapped.state_topic.is_some()
                && entry
                    .model
                    .entity(mapped.uid)
                    .and_then(|entity| entry.model.enum_type_of(entity))
                    .is_some_and(|enum_type| {
                        enum_type
                            .declared_values
                            .iter()
                            .any(|value| enum_type.member(*value).is_some())
                    })
        })
        .expect("the dishwasher declares a readable enumeration with a named member");
    let entity = entry.model.entity(mapped.uid).expect("in the model");
    let enum_type = entry.model.enum_type_of(entity).expect("an enumeration");
    let ordinal = *enum_type
        .declared_values
        .iter()
        .find(|value| enum_type.member(**value).is_some())
        .expect("checked above");
    let label = enum_type.member(ordinal).expect("checked above").to_owned();
    (mapped.uid, ordinal, label)
}

#[test]
fn an_entity_descriptor_carries_its_current_value() {
    let state = state();
    let (uid, _ordinal, _label) = enum_feature(&state);

    // What a device task does: register the shadow, then seed it from the appliance's own report.
    let shadow = Arc::new(Mutex::new(Shadow::new()));
    state.shadows().install(SLUG, Arc::clone(&shadow));
    shadow.lock().seed(&[ValueUpdate {
        uid: u64::from(uid),
        value: json!(0),
        description: hc_proto::DescriptionUpdate::default(),
    }]);

    let entry = state.devices().entry(SLUG).expect("registered");
    let descriptors = entry.entity_descriptors(state.shadows().shadow(SLUG).as_deref());
    let described = descriptors
        .iter()
        .find(|descriptor| descriptor.uid == u32::from(uid))
        .expect("the feature is described");

    assert!(
        described.value.is_some(),
        "a feature whose value the appliance has reported must carry it on its descriptor — the \
         dashboard has nothing to render otherwise: {described:?}"
    );
}

#[test]
fn an_enum_value_is_rendered_as_its_declared_label() {
    let state = state();
    let (uid, ordinal, label) = enum_feature(&state);

    let shadow = Arc::new(Mutex::new(Shadow::new()));
    state.shadows().install(SLUG, Arc::clone(&shadow));
    shadow.lock().seed(&[ValueUpdate {
        uid: u64::from(uid),
        value: json!(ordinal),
        description: hc_proto::DescriptionUpdate::default(),
    }]);

    let entry = state.devices().entry(SLUG).expect("registered");
    let descriptors = entry.entity_descriptors(state.shadows().shadow(SLUG).as_deref());
    let described = descriptors
        .iter()
        .find(|descriptor| descriptor.uid == u32::from(uid))
        .expect("described");

    assert_eq!(
        described.value.as_deref(),
        Some(label.as_str()),
        "the appliance reports the wire ordinal {ordinal}; the descriptor must carry the DECLARED \
         LABEL, because the same descriptor's `options` are labels and a select can only match those"
    );
    assert!(
        described.options.contains(&label),
        "and the rendered value must be one of the options the SPA was given: {described:?}"
    );
}

#[test]
fn a_device_with_no_task_reports_no_values_rather_than_empty_ones() {
    let state = state();
    assert!(
        state.shadows().shadow(SLUG).is_none(),
        "no task was started, so there is no shadow — the precondition of this test"
    );

    let entry = state.devices().entry(SLUG).expect("registered");
    let descriptors = entry.entity_descriptors(None);
    assert!(
        !descriptors.is_empty(),
        "the catalogue is still served: the features exist whether or not the appliance is reachable"
    );
    assert!(
        descriptors
            .iter()
            .all(|descriptor| descriptor.value.is_none()),
        "every value must be ABSENT, not empty. A disconnected appliance rendering as one whose \
         features all read '' is the failure ShadowRegistry's docs were written against"
    );
}

/// A value update reaches an SSE subscriber, on its own `event:` kind.
///
/// The device task publishes each rendered value onto the trace bus, which is the SSE stream's
/// source and has a retained ring buffer, so a dashboard opened after a device connected still sees
/// its values. Asserted through `TraceFrame::from_event`, which is what the stream delivers.
#[test]
fn an_sse_value_event_carries_the_device_uid_and_rendered_value() {
    use server::types::{
        SseEventKind, TraceFrame, VALUE_DEVICE_FIELD, VALUE_FIELD, VALUE_UID_FIELD,
    };

    let bus = common::bus();
    let mut subscriber = bus.subscribe();

    // What the device task's sink does for each decoded update.
    bus.emit(
        hc_core::trace::TraceRecord::new(
            hc_core::trace::TraceLevel::Debug,
            "kessel::values",
            "feature value",
        )
        .with_field(VALUE_DEVICE_FIELD, SLUG)
        .with_field(VALUE_UID_FIELD, "41")
        .with_field(VALUE_FIELD, "Relative"),
    );

    let event = subscriber.try_recv().expect("the subscriber received it");
    let frame = TraceFrame::from_event(event);

    assert_eq!(
        frame.kind,
        SseEventKind::Value,
        "it must arrive on the `value` event, not as a log line — the SPA selects on the event kind"
    );
    assert_eq!(
        frame.fields.get(VALUE_DEVICE_FIELD).map(String::as_str),
        Some(SLUG),
        "the device is named by its slug, which carries no identifier"
    );
    assert_eq!(
        frame.fields.get(VALUE_UID_FIELD).map(String::as_str),
        Some("41"),
        "and the feature by its uid, so the SPA can match it to a descriptor"
    );
    assert_eq!(
        frame.fields.get(VALUE_FIELD).map(String::as_str),
        Some("Relative"),
        "carrying the RENDERED value — the same form the descriptor snapshot and MQTT use, so the SPA \
         never has to know there are three renderings"
    );
}

/// A log line that happens to carry a `uid` is not a value event.
///
/// The marker field is `feature_value`, not `uid`: `uid` is a plausible field on a future log line,
/// and one misclassified as a value event would put arbitrary prose where the SPA expects a value.
#[test]
fn a_log_line_with_a_uid_is_not_a_value_event() {
    use server::types::{SseEventKind, TraceFrame, VALUE_UID_FIELD};

    let bus = common::bus();
    let mut subscriber = bus.subscribe();
    bus.emit(
        hc_core::trace::TraceRecord::new(
            hc_core::trace::TraceLevel::Warn,
            "kessel::something",
            "a uid appeared in a log line",
        )
        .with_field(VALUE_UID_FIELD, "41"),
    );
    let frame = TraceFrame::from_event(subscriber.try_recv().expect("received"));
    assert_eq!(
        frame.kind,
        SseEventKind::Log,
        "only the dedicated marker field makes a value event: {frame:?}"
    );
}

/// **A description change reaches the registry, so both surfaces see the appliance's newer word.**
///
/// A running appliance revises `access`, `available` and the numeric
/// bounds with `NOTIFY /ro/descriptionChange`. `hc_proto::apply_updates` carries them and
/// `hc_core::mapping::LiveDescriptions` overlays them — but the mapping the dashboard reads and the
/// mapping the MQTT bridge announces both live in this registry entry, so nothing changes until the
/// entry absorbs one.
///
/// The return value is what stops this being a broker storm: most batches are values only, and a
/// batch that revised nothing must not rebuild a mapping or trigger a re-announce of 288 configs.
#[test]
fn a_description_change_rebuilds_the_entry_that_both_surfaces_read() {
    let state = state();
    let uid = numeric_feature(&state);

    let unchanged = state
        .devices()
        .update(SLUG, |entry| {
            entry.absorb_descriptions(
                SLUG,
                &[ValueUpdate {
                    uid: u64::from(uid),
                    value: json!(1),
                    description: hc_proto::DescriptionUpdate::default(),
                }],
            )
        })
        .expect("registered");
    assert!(
        !unchanged,
        "a value-only batch revises no description and must not report a change"
    );

    let changed = state
        .devices()
        .update(SLUG, |entry| {
            entry.absorb_descriptions(
                SLUG,
                &[ValueUpdate {
                    uid: u64::from(uid),
                    value: json!(1),
                    description: hc_proto::DescriptionUpdate {
                        min: Some(-24),
                        max: Some(-16),
                        step: Some(1),
                        ..hc_proto::DescriptionUpdate::default()
                    },
                }],
            )
        })
        .expect("registered");
    assert!(changed, "bounds the appliance stated are a change");

    let entry = state.devices().entry(SLUG).expect("registered");
    let mapped = entry
        .mapping
        .entries()
        .iter()
        .find(|mapped| mapped.uid == uid)
        .expect("the feature is still mapped");
    assert_eq!(
        mapped.number_range,
        Some(hc_profile::NumericRange {
            min: Some(-24),
            max: Some(-16),
            step: Some(1),
        }),
        "the rebuilt mapping carries the bounds the appliance stated, not the archive's"
    );
}

/// The uid of a writable numeric feature on the dishwasher — one whose bounds a control would use.
fn numeric_feature(state: &AppState) -> u16 {
    let entry = state.devices().entry(SLUG).expect("registered");
    entry
        .mapping
        .entries()
        .iter()
        .find(|mapped| mapped.class == hc_core::mapping::EntityClass::Number)
        .map(|mapped| mapped.uid)
        .expect("the dishwasher declares a writable numeric feature")
}
